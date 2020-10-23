pub mod constants;
pub mod gas_mixture;
use dm::*;
use std::collections::HashMap;

struct Gases<'a> {
	pub gas_ids: HashMap<u32, u32>,
	pub gas_specific_heat: Vec<f32>,
	pub gas_id_to_type: Vec<Value<'a>>,
	pub total_num_gases: u32,
}

lazy_static! {
	static ref GAS_INFO: Gases<'static> = {
		let gas_types_list: dm::List =
			dm::List::from(Proc::find("/proc/gas_types").unwrap().call(&[]).unwrap());
		let mut gas_ids: HashMap<u32, u32> = HashMap::new();
		let mut gas_specific_heat: Vec<f32> = Vec::new();
		let mut gas_id_to_type: Vec<Value> = Vec::new();
		let total_num_gases: u32 = gas_types_list.len();
		for i in 0..total_num_gases {
			let v = gas_types_list.get(i).unwrap();
			unsafe {
				gas_ids.insert(v.value.data.id, i);
			}
			gas_specific_heat.push(v.as_number().unwrap_or(20.0));
			gas_id_to_type.push(v);
		}
		Gases {
			gas_ids,
			gas_specific_heat,
			gas_id_to_type,
			total_num_gases,
		}
	};
}

/// Returns a static reference to a vector of all the specific heats of the gases.
pub fn gas_specific_heats() -> &'static Vec<f32> {
	&GAS_INFO.gas_specific_heat
}

/// Returns a reference to the total number of gases findable. Only used by gas mixtures; should probably stay that way.
pub fn total_num_gases() -> &'static u32 {
	&GAS_INFO.total_num_gases
}
