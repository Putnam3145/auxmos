use auxtools::*;

use std::cell::RefCell;

use super::gas_mixture::{FlatSimdVec, GasMixture};

use super::{gas_idx_to_id, total_num_gases};

use core::cmp::Ordering;

#[derive(Clone)]
pub struct Reaction {
	id: ReactionIdentifier,
	min_temp_req: Option<f32>,
	max_temp_req: Option<f32>,
	min_ener_req: Option<f32>,
	min_fire_req: Option<f32>,
	min_gas_reqs: FlatSimdVec,
}

#[derive(Copy, Clone)]
pub struct ReactionIdentifier {
	string_id_hash: u64,
	priority: f32,
}

impl std::hash::Hash for ReactionIdentifier {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		self.string_id_hash.hash(state);
	}
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

use std::collections::BTreeMap;

thread_local! {
	// gotta be a BTreeMap for priorities
	static REACTION_VALUES: RefCell<BTreeMap<ReactionIdentifier,Value>> = RefCell::new(BTreeMap::new())
}

#[shutdown]
fn clean_up_reaction_values() {
	REACTION_VALUES.with(|reaction_values| {
		reaction_values.borrow_mut().clear();
	})
}

pub fn react_by_id(id: ReactionIdentifier, src: &Value, holder: &Value) -> DMResult {
	REACTION_VALUES.with(|r| {
		r.borrow()
			.get(&id)
			.ok_or_else(|| runtime!("Reaction with invalid id"))
			.and_then(|reaction| reaction.call("react", &[src, holder]))
	})
}

impl Reaction {
	/// Takes a /datum/gas_reaction and makes a Rust reaction out of it.
	///  This will panic if it's given anything that isn't a /datum/gas_reaction.
	///  Yes, *panic*, not runtime. This is intentional. Please do not give it
	///  anything but a /datum/gas_reaction.
	pub fn from_byond_reaction(reaction: &Value) -> Self {
		let priority = reaction.get_number(byond_string!("priority")).unwrap();
		let string_id_hash =
			fxhash::hash64(reaction.get_string(byond_string!("id")).unwrap().as_bytes());
		let id = ReactionIdentifier {
			string_id_hash,
			priority,
		};
		let our_reaction = {
			if let Ok(min_reqs) = reaction.get_list(byond_string!("min_requirements")) {
				let mut min_gas_reqs: FlatSimdVec = FlatSimdVec::new();
				for i in 0..total_num_gases() {
					if let Ok(gas_req) =
						min_reqs.get(Value::from_string(&*gas_idx_to_id(i).unwrap()).unwrap())
					{
						if let Ok(req_amount) = gas_req.as_number() {
							min_gas_reqs.force_set(i, req_amount);
						}
					}
				}
				let min_temp_req = min_reqs
					.get(&Value::from_string("TEMP").unwrap_or(Value::null()))
					.unwrap_or(Value::null())
					.as_number()
					.ok();
				let max_temp_req = min_reqs
					.get(&Value::from_string("MAX_TEMP").unwrap_or(Value::null()))
					.unwrap_or(Value::null())
					.as_number()
					.ok();
				let min_ener_req = min_reqs
					.get(&Value::from_string("ENER").unwrap_or(Value::null()))
					.unwrap_or(Value::null())
					.as_number()
					.ok();
				let min_fire_req = min_reqs
					.get(&Value::from_string("FIRE_REAGENTS").unwrap_or(Value::null()))
					.unwrap_or(Value::null())
					.as_number()
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
					max_temp_req: Some(-1.0),
					min_ener_req: None,
					min_fire_req: None,
					min_gas_reqs: FlatSimdVec::new(),
				}
			}
		};
		REACTION_VALUES.with(|r| r.borrow_mut().insert(our_reaction.id, reaction.clone()));
		our_reaction
	}
	pub fn get_id(&self) -> ReactionIdentifier {
		self.id
	}
	/// Checks if the given gas mixture can react with this reaction.
	pub fn check_conditions(&self, mix: &GasMixture) -> bool {
		use itertools::{
			EitherOrBoth::{Both, Left, Right},
			Itertools,
		};
		self.min_temp_req
			.map_or(true, |temp_req| mix.get_temperature() >= temp_req)
			&& self
				.max_temp_req
				.map_or(true, |temp_req| mix.get_temperature() <= temp_req)
			&& self
				.min_ener_req
				.map_or(true, |ener_req| mix.thermal_energy() >= ener_req)
			&& self.min_fire_req.map_or(true, |fire_req| {
				mix.get_fuel_amount() > fire_req && mix.get_oxidation_power() > fire_req
				})
			&& self
			.min_gas_reqs
			.iter()
			.zip_longest(mix.gases().iter().copied())
			.all(|pair| {
				match pair {
					Left(_) => false, // they have more gases than us
					Right(_) => true, // we have more gases than them
					Both(a,b) => a.le(b).all()
				}
			})
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
