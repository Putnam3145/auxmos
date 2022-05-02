#[cfg(feature = "reaction_hooks")]
pub mod hooks;

use auxtools::*;

use std::cell::RefCell;

use crate::gas::{gas_idx_to_id, total_num_gases, GasIDX, Mixture};

use core::cmp::Ordering;

#[derive(Clone)]
pub struct Reaction {
	id: ReactionIdentifier,
	min_temp_req: Option<f32>,
	max_temp_req: Option<f32>,
	min_ener_req: Option<f32>,
	min_fire_req: Option<f32>,
	min_gas_reqs: Vec<(GasIDX, f32)>,
}

#[derive(Copy, Clone, Default)]
pub struct ReactionIdentifier {
	string_id_hash: u64,
	priority: f32,
}

impl Ord for ReactionIdentifier {
	fn cmp(&self, other: &Self) -> Ordering {
		self.priority
			.partial_cmp(&other.priority)
			.unwrap_or_else(|| self.string_id_hash.cmp(&other.string_id_hash))
	}
}

impl PartialOrd for ReactionIdentifier {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl PartialEq for ReactionIdentifier {
	fn eq(&self, other: &Self) -> bool {
		self.string_id_hash == other.string_id_hash
	}
}

impl Eq for ReactionIdentifier {}

impl Ord for Reaction {
	fn cmp(&self, other: &Self) -> Ordering {
		self.id.cmp(&other.id)
	}
}

impl PartialOrd for Reaction {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl PartialEq for Reaction {
	fn eq(&self, other: &Self) -> bool {
		self.id == other.id
	}
}

impl Eq for Reaction {}

use std::collections::BTreeMap;

enum ReactionSide {
	ByondSide(Value),
	RustSide(fn(&Value, &Value) -> DMResult<Value>),
}

thread_local! {
	// gotta be a BTreeMap for priorities
	static REACTION_VALUES: RefCell<BTreeMap<ReactionIdentifier,ReactionSide>> = RefCell::new(BTreeMap::new())
}

#[shutdown]
fn clean_up_reaction_values() {
	REACTION_VALUES.with(|reaction_values| {
		reaction_values.borrow_mut().clear();
	});
}

pub fn react_by_id(id: &ReactionIdentifier, src: &Value, holder: &Value) -> DMResult {
	REACTION_VALUES.with(|r| {
		r.borrow().get(id).map_or_else(
			|| Err(runtime!("Reaction with invalid id")),
			|reaction| match reaction {
				ReactionSide::ByondSide(val) => val.call("react", &[src, holder]),
				ReactionSide::RustSide(func) => func(src, holder),
			},
		)
	})
}

impl Reaction {
	/// Takes a `/datum/gas_reaction` and makes a byond reaction out of it.
	///
	/// # Panics
	///
	///
	/// If given anything but a `/datum/gas_reaction`, this will panic.
	pub fn from_byond_reaction(reaction: &Value) -> Self {
		let priority = -reaction
			.get_number(byond_string!("priority"))
			.unwrap_or_default();
		let string_id = reaction
			.get_string(byond_string!("id"))
			.unwrap_or_else(|_| "invalid".to_string());
		let func = {
			#[cfg(feature = "reaction_hooks")]
			{
				hooks::func_from_id(string_id.as_str())
			}
			#[cfg(not(feature = "reaction_hooks"))]
			{
				None
			}
		};
		let string_id_hash = fxhash::hash64(string_id.as_bytes());
		let id = ReactionIdentifier {
			string_id_hash,
			priority,
		};
		let our_reaction = {
			if let Ok(min_reqs) = reaction.get_list(byond_string!("min_requirements")) {
				let mut min_gas_reqs: Vec<(GasIDX, f32)> = Vec::new();
				for i in 0..total_num_gases() {
					if let Ok(req_amount) = min_reqs
						.get(gas_idx_to_id(i).unwrap_or_else(|_| Value::null()))
						.and_then(|v| v.as_number())
					{
						min_gas_reqs.push((i, req_amount));
					}
				}
				let min_temp_req = min_reqs
					.get(byond_string!("TEMP"))
					.and_then(|v| v.as_number())
					.ok();
				let max_temp_req = min_reqs
					.get(byond_string!("MAX_TEMP"))
					.and_then(|v| v.as_number())
					.ok();
				let min_ener_req = min_reqs
					.get(byond_string!("ENER"))
					.and_then(|v| v.as_number())
					.ok();
				let min_fire_req = min_reqs
					.get(byond_string!("FIRE_REAGENTS"))
					.and_then(|v| v.as_number())
					.ok();
				Reaction {
					id,
					min_temp_req,
					max_temp_req,
					min_ener_req,
					min_fire_req,
					min_gas_reqs,
				}
			} else {
				Reaction {
					id,
					min_temp_req: None,
					max_temp_req: Some(1.0),
					min_ener_req: None,
					min_fire_req: None,
					min_gas_reqs: vec![],
				}
			}
		};
		if let Some(function) = func {
			REACTION_VALUES.with(|r| {
				r.borrow_mut()
					.insert(our_reaction.id, ReactionSide::RustSide(function))
			});
		} else {
			REACTION_VALUES.with(|r| {
				r.borrow_mut()
					.insert(our_reaction.id, ReactionSide::ByondSide(reaction.clone()))
			});
		}
		our_reaction
	}
	pub fn get_id(&self) -> ReactionIdentifier {
		self.id
	}
	/// Checks if the given gas mixture can react with this reaction.
	pub fn check_conditions(&self, mix: &Mixture) -> bool {
		self.min_temp_req
			.map_or(true, |temp_req| mix.get_temperature() >= temp_req)
			&& self
				.max_temp_req
				.map_or(true, |temp_req| mix.get_temperature() <= temp_req)
			&& self
				.min_gas_reqs
				.iter()
				.all(|&(k, v)| mix.get_moles(k) >= v)
			&& self
				.min_ener_req
				.map_or(true, |ener_req| mix.thermal_energy() >= ener_req)
			&& self.min_fire_req.map_or(true, |fire_req| {
				let (oxi, fuel) = mix.get_burnability();
				oxi.min(fuel) >= fire_req
			})
	}
	/// Returns the priority of the reaction.
	pub fn get_priority(&self) -> f32 {
		self.id.priority
	}
	/// Calls the reaction with the given arguments.
	pub fn react(&self, src: &Value, holder: &Value) -> DMResult {
		react_by_id(&self.id, src, holder)
	}
}
