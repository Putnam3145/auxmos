use std::collections::BTreeMap;

use auxtools::*;

use std::cell::RefCell;

use super::gas_mixture::GasMixture;

use super::{gas_id_to_type, total_num_gases};

use core::cmp::Ordering;

#[derive(Clone)]
pub struct Reaction {
	id: std::string::String,
	priority: f32,
	min_temp_req: Option<f32>,
	max_temp_req: Option<f32>,
	min_ener_req: Option<f32>,
	min_gas_reqs: BTreeMap<usize, f32>,
}

impl Ord for Reaction {
	fn cmp(&self, other: &Self) -> Ordering {
		self.id.cmp(&other.id)
	}
}

impl PartialEq for Reaction {
	fn eq(&self, other: &Self) -> bool {
		self.id == other.id
	}
}

impl Eq for Reaction {}

impl PartialOrd for Reaction {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

thread_local! {static REACTION_VALUES: RefCell<BTreeMap<Reaction,Value>> = RefCell::new(BTreeMap::new())}

impl Reaction {
	/// Takes a /datum/reaction and makes a byond reaction out of it.
	///  This will panic if it's given anything that isn't a /datum/reaction.
	///  Yes, *panic*, not runtime. This is intentional. Please do not give it
	///  anything but a /datum/reaction.
	pub fn from_byond_reaction(reaction: &Value) -> Self {
		let min_reqs = reaction.get_list("min_requirements").unwrap();
		let mut min_gas_reqs: BTreeMap<usize, f32> = BTreeMap::new();
		for i in 0..total_num_gases() {
			if let Ok(gas_req) = min_reqs.get(&gas_id_to_type(i).unwrap()) {
				if let Ok(req_amount) = gas_req.as_number() {
					min_gas_reqs.insert(i, req_amount);
				}
			}
		}
		let min_temp_req = min_reqs
			.get(&Value::from_string("TEMP"))
			.unwrap_or(Value::null())
			.as_number()
			.ok();
		let max_temp_req = min_reqs
			.get(&Value::from_string("MAX_TEMP"))
			.unwrap_or(Value::null())
			.as_number()
			.ok();
		let min_ener_req = min_reqs
			.get(&Value::from_string("ENER"))
			.unwrap_or(Value::null())
			.as_number()
			.ok();
		let priority = reaction.get_number("priority").unwrap();
		let id = reaction.get_string("id").unwrap();
		let our_reaction = Reaction {
			id,
			priority,
			min_temp_req,
			max_temp_req,
			min_ener_req,
			min_gas_reqs,
		};
		REACTION_VALUES.with(|r| {
			r.borrow_mut()
				.insert(our_reaction.clone(), reaction.clone())
		});
		our_reaction
	}
	/// Checks if the given gas mixture can react with this reaction.
	pub fn check_conditions(&self, mix: &GasMixture) -> bool {
		if self.min_temp_req.is_some() && mix.get_temperature() < self.min_temp_req.unwrap() {
			return false;
		}
		if self.max_temp_req.is_some() && mix.get_temperature() > self.max_temp_req.unwrap() {
			return false;
		}
		if self.min_ener_req.is_some() && mix.thermal_energy() < self.min_ener_req.unwrap() {
			return false;
		}
		self.min_gas_reqs
			.iter()
			.all(|(&k, &v)| mix.get_moles(k) >= v)
	}
	/// Returns the priority of the reaction.
	pub fn get_priority(&self) -> f32 {
		self.priority
	}
	/// Calls the reaction with the given arguments.
	pub fn react(&self, src: &Value, holder: &Value) -> DMResult {
		REACTION_VALUES.with(|r| {
			if let Some(reaction) = r.borrow().get(self) {
				reaction.call("react", &[src, holder])
			} else {
				Err(runtime!("Reaction with invalid id: {}", self.id))
			}
		})
	}
}
