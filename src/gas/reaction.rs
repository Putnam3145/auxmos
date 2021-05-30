use auxtools::*;

use std::cell::RefCell;

use super::gas_mixture::GasMixture;

use super::{gas_id_to_type, total_num_gases};

use core::cmp::Ordering;

use bitvec::prelude::*;

#[derive(Clone)]
pub struct Reaction {
	id: ReactionIdentifier,
	min_temp_req: Option<f32>,
	max_temp_req: Option<f32>,
	min_ener_req: Option<f32>,
	req_gases: BitVec<Lsb0, u8>,
	min_gas_reqs: Vec<f32>,
}

#[derive(Copy, Clone)]
pub struct ReactionIdentifier {
	string_id_hash: u64,
	priority: f32,
}

impl Ord for ReactionIdentifier {
	fn cmp(&self, other: &Self) -> Ordering {
		if let Some(priority_cmp) = self.priority.partial_cmp(&other.priority) {
			priority_cmp
		} else {
			self.string_id_hash.cmp(&other.string_id_hash)
		}
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

thread_local! {
	static REACTION_VALUES: RefCell<std::collections::BTreeMap<ReactionIdentifier,Value>> = RefCell::new(std::collections::BTreeMap::new())
}

pub fn react_by_id(id: ReactionIdentifier, src: &Value, holder: &Value) -> DMResult {
	REACTION_VALUES.with(|r| {
		if let Some(reaction) = r.borrow().get(&id) {
			reaction.call("react", &[src, holder])
		} else {
			Err(runtime!("Reaction with invalid id"))
		}
	})
}

impl Reaction {
	/// Takes a /datum/reaction and makes a byond reaction out of it.
	///  This will panic if it's given anything that isn't a /datum/reaction.
	///  Yes, *panic*, not runtime. This is intentional. Please do not give it
	///  anything but a /datum/reaction.
	pub fn from_byond_reaction(reaction: &Value) -> Self {
		let min_reqs = reaction
			.get_list(byond_string!("min_requirements"))
			.unwrap();
		let mut min_gas_reqs = Vec::new();
		let mut req_gases = BitVec::new();
		req_gases.resize(total_num_gases() + 1, false);
		for i in 0..total_num_gases() {
			if let Ok(gas_req) = min_reqs.get(&gas_id_to_type(i).unwrap()) {
				if let Ok(req_amount) = gas_req.as_number() {
					min_gas_reqs.push(req_amount);
					req_gases.set(i, true);
				}
			}
		}
		req_gases.truncate(req_gases.len() - req_gases.trailing_zeros());
		req_gases.shrink_to_fit();
		let min_temp_req = min_reqs
			.get(byond_string!("TEMP"))
			.unwrap_or_else(|_| Value::null())
			.as_number()
			.ok();
		let max_temp_req = min_reqs
			.get(byond_string!("MAX_TEMP"))
			.unwrap_or_else(|_| Value::null())
			.as_number()
			.ok();
		let min_ener_req = min_reqs
			.get(byond_string!("ENER"))
			.unwrap_or_else(|_| Value::null())
			.as_number()
			.ok();
		let priority = reaction.get_number(byond_string!("priority")).unwrap();
		use std::hash::Hasher;
		let mut hasher = std::collections::hash_map::DefaultHasher::new();
		hasher.write(reaction.get_string(byond_string!("id")).unwrap().as_bytes());
		let string_id_hash = hasher.finish();
		let id = ReactionIdentifier {
			string_id_hash,
			priority,
		};
		let our_reaction = Reaction {
			id,
			min_temp_req,
			max_temp_req,
			min_ener_req,
			req_gases,
			min_gas_reqs,
		};
		REACTION_VALUES.with(|r| r.borrow_mut().insert(our_reaction.id, reaction.clone()));
		our_reaction
	}
	pub fn get_id(&self) -> ReactionIdentifier {
		self.id
	}
	/// Checks if the given gas mixture can react with this reaction.
	pub fn check_conditions(&self, mix: &GasMixture) -> bool {
		self.min_temp_req
			.map_or(true, |temp_req| mix.get_temperature() >= temp_req)
			&& self
				.max_temp_req
				.map_or(true, |temp_req| mix.get_temperature() <= temp_req)
			&& self
				.min_ener_req
				.map_or(true, |ener_req| mix.thermal_energy() >= ener_req)
			&& self
				.req_gases
				.iter_ones()
				.zip(self.min_gas_reqs.iter())
				.all(|(k, &v)| mix.get_moles(k) >= v)
	}
	/// Returns the priority of the reaction.
	pub fn get_priority(&self) -> f32 {
		self.id.priority
	}
	/// Calls the reaction with the given arguments.
	pub fn react(&self, src: &Value, holder: &Value) -> DMResult {
		react_by_id(self.id, src, holder)
	}
}
