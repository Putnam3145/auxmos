#[cfg(feature = "reaction_hooks")]
mod hooks;

use byondapi::{prelude::*, typecheck_trait::ByondTypeCheck};

use crate::gas::{gas_idx_to_id, total_num_gases, GasIDX, Mixture};

use std::cell::RefCell;

use float_ord::FloatOrd;

pub type ReactionPriority = FloatOrd<f32>;

pub type ReactionIdentifier = u64;

use eyre::{Context, Result};

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
use hashbrown::HashMap;

enum ReactionSide {
	ByondSide(ByondValue),
	RustSide(fn(ByondValue, ByondValue) -> Result<ByondValue>),
}

thread_local! {
	static REACTION_VALUES: RefCell<HashMap<ReactionIdentifier, ReactionSide, FxBuildHasher>> = Default::default();
}

pub fn clean_up_reaction_values() {
	crate::turfs::wait_for_tasks();
	REACTION_VALUES.with(|reaction_values| {
		reaction_values.borrow_mut().clear();
	});
}

/// Runs a reaction given a `ReactionIdentifier`. Returns the result of the reaction, error or success.
/// # Errors
/// If the reaction itself has a runtime.
pub fn react_by_id(
	id: ReactionIdentifier,
	src: ByondValue,
	holder: ByondValue,
) -> Result<ByondValue> {
	REACTION_VALUES.with(|r| {
		r.borrow().get(&id).map_or_else(
			|| Err(eyre::eyre!("Reaction with invalid id")),
			|reaction| match reaction {
				ReactionSide::ByondSide(val) => val
					.call("react", &[src, holder])
					.wrap_err("calling byond side react in react_by_id"),
				ReactionSide::RustSide(func) => func(src, holder),
			},
		)
	})
}

impl Reaction {
	/// Takes a `/datum/gas_reaction` and makes a byond reaction out of it.
	pub fn from_byond_reaction(reaction: ByondValue) -> Result<Self> {
		let priority = FloatOrd(
			reaction
				.read_number("priority")
				.map_err(|_| eyre::eyre!("Reaction priority must be a number!"))?,
		);
		let string_id = reaction
			.read_string("id")
			.map_err(|_| eyre::eyre!("Reaction id must be a string!"))?;
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
			if let Some(min_reqs) = reaction
				.read_var("min_requirements")
				.map_or(None, |value| value.is_list().then_some(value))
			{
				let mut min_gas_reqs: Vec<(GasIDX, f32)> = Vec::new();
				for i in 0..total_num_gases() {
					if let Ok(req_amount) = min_reqs
						.read_list_index(gas_idx_to_id(i))
						.and_then(|v| v.get_number())
					{
						min_gas_reqs.push((i, req_amount));
					}
				}
				let min_temp_req = min_reqs
					.read_list_index("TEMP")
					.ok()
					.map(|item| item.get_number().ok())
					.flatten();
				let max_temp_req = min_reqs
					.read_list_index("MAX_TEMP")
					.ok()
					.map(|item| item.get_number().ok())
					.flatten();
				let min_ener_req = min_reqs
					.read_list_index("ENER")
					.ok()
					.map(|item| item.get_number().ok())
					.flatten();
				let min_fire_req = min_reqs
					.read_list_index("FIRE_REAGENTS")
					.ok()
					.map(|item| item.get_number().ok())
					.flatten();
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
				Err(eyre::eyre!(format!(
					"Reaction {string_id} doesn't have a gas requirements list!"
				)))
			}
		}?;

		REACTION_VALUES.with(|r| -> Result<()> {
			let mut reaction_map = r.borrow_mut();
			match func {
				Some(function) => {
					reaction_map.insert(our_reaction.id, ReactionSide::RustSide(function))
				}
				None => {
					reaction_map.insert(our_reaction.id, ReactionSide::ByondSide(reaction.clone()))
				}
			};
			Ok(())
		})?;
		Ok(our_reaction)
	}
	/// Gets the reaction's identifier.
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
	pub fn react(&self, src: ByondValue, holder: ByondValue) -> Result<ByondValue> {
		react_by_id(self.id, src, holder)
	}
}
