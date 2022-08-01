#[cfg(feature = "reaction_hooks")]
mod hooks;

use auxtools::{byond_string, runtime, shutdown, DMResult, Runtime, Value};

use crate::gas::{gas_idx_to_id, total_num_gases, GasIDX, Mixture};

use std::cell::RefCell;

type ReactionPriority = u32;

type ReactionIdentifier = u64;

#[derive(Clone)]
pub struct Reaction {
	id: ReactionIdentifier,
	priority: ReactionPriority,
	min_temp_req: Option<f32>,
	max_temp_req: Option<f32>,
	min_ener_req: Option<f32>,
	min_fire_req: Option<f32>,
	min_gas_reqs: Vec<(GasIDX, f32)>,
}

use fxhash::FxBuildHasher;
use std::collections::HashMap;

enum ReactionSide {
	ByondSide(Value),
	RustSide(fn(&Value, &Value) -> DMResult<Value>),
}

thread_local! {
	static REACTION_VALUES: RefCell<HashMap<ReactionIdentifier, ReactionSide, FxBuildHasher>> = Default::default();
}

#[shutdown]
fn clean_up_reaction_values() {
	crate::turfs::wait_for_tasks();
	REACTION_VALUES.with(|reaction_values| {
		reaction_values.borrow_mut().clear();
	});
}

/// Runs a reaction given a `ReactionIdentifier`. Returns the result of the reaction, error or success.
/// # Errors
/// If the reaction itself has a runtime.
pub fn react_by_id(id: ReactionIdentifier, src: &Value, holder: &Value) -> DMResult {
	REACTION_VALUES.with(|r| {
		r.borrow().get(&id).map_or_else(
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
	#[must_use]
	pub fn from_byond_reaction(reaction: &Value) -> Result<Self, Runtime> {
		let priority = reaction
			.get_number(byond_string!("priority"))
			.map_err(|_| runtime!("Reaction priorty must be a number!"))?
			.floor() as u32;
		let string_id = reaction
			.get_string(byond_string!("id"))
			.map_err(|_| runtime!("Reaction id must be a string!"))?;
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
		let id = fxhash::hash64(string_id.as_bytes());
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
				if min_gas_reqs.len() == 0 {
					return Err(runtime!(
						"Tried to register a reaction with no valid requirements!"
					));
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
				Ok(Reaction {
					id,
					priority,
					min_temp_req,
					max_temp_req,
					min_ener_req,
					min_fire_req,
					min_gas_reqs,
				})
			} else {
				Err(runtime!("Reactions must have a gas requirements list!"))
			}
		}?;

		REACTION_VALUES.with(|r| -> Result<(), Runtime> {
			let reaction_map = r.borrow_mut();
			if reaction_map.contains_key(&our_reaction.id) {
				return Err(runtime!(format!(
					"Duplicate reaction id {}, only one reaction of this id will be registered",
					string_id
				)));
			}
			match func {
				Some(function) => r
					.borrow_mut()
					.insert(our_reaction.id, ReactionSide::RustSide(function)),
				None => r
					.borrow_mut()
					.insert(our_reaction.id, ReactionSide::ByondSide(reaction.clone())),
			};
			Ok(())
		})?;
		Ok(our_reaction)
	}
	#[must_use]
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
	#[must_use]
	pub fn get_priority(&self) -> ReactionPriority {
		self.priority
	}
	/// Calls the reaction with the given arguments.
	/// # Errors
	/// If the reaction itself has a runtime error, this will propagate it up.
	pub fn react(&self, src: &Value, holder: &Value) -> DMResult {
		react_by_id(self.id, src, holder)
	}
}
