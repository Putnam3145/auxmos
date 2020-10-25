pub mod constants;
pub mod gas_mixture;
use dm::*;
use std::collections::HashMap;

struct Gases<'a> {
	pub gas_ids: HashMap<u32, usize>,
	pub gas_specific_heat: Vec<f32>,
	pub gas_id_to_type: Vec<Value<'a>>,
	pub total_num_gases: usize,
}

lazy_static! {
	static ref GAS_INFO: Gases<'static> = {
		let gas_types_list: dm::List =
			dm::List::from(Proc::find("/proc/gas_types").unwrap().call(&[]).unwrap());
		let mut gas_ids: HashMap<u32, usize> = HashMap::new();
		let mut gas_specific_heat: Vec<f32> = Vec::new();
		let mut gas_id_to_type: Vec<Value> = Vec::new();
		let total_num_gases: usize = gas_types_list.len() as usize;
		for i in 0..total_num_gases {
			let v = gas_types_list.get(i as u32).unwrap();
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

/// Returns the total number of gases in use. Only used by gas mixtures; should probably stay that way.
pub fn total_num_gases() -> usize {
	GAS_INFO.total_num_gases
}

pub fn gas_id_from_type(path: &Value) -> Result<usize, Runtime> {
	let id: u32;
	unsafe {
		id = path.value.data.id;
	}
	if !GAS_INFO.gas_ids.contains_key(&id) {
		Err(runtime!(
			"Invalid type! This should be a gas datum typepath!"
		))
	} else {
		Ok(*GAS_INFO.gas_ids.get(&id).unwrap())
	}
}

pub fn gas_id_to_type(id: usize) -> Result<Value<'static>, Runtime> {
	if GAS_INFO.gas_id_to_type.len() < id {
		Ok(GAS_INFO.gas_id_to_type[id].clone())
	} else {
		Err(runtime!(format!("Invalid gas ID: {}", id)))
	}
}

use gas_mixture::GasMixture;

use std::sync::RwLock;

pub struct GasMixtures {}

lazy_static! {
	static ref GAS_MIXTURES: RwLock<Vec<GasMixture>> = RwLock::new(Vec::with_capacity(200000));
}
lazy_static! {
	static ref NEXT_GAS_IDS: RwLock<Vec<usize>> = RwLock::new(Vec::new());
}

impl GasMixtures {
	fn get_gas_mix_by_id(id: usize) -> &'static GasMixture {
		GAS_MIXTURES.read().unwrap().get(id).unwrap()
	}
	fn get_gas_mix_by_id_mut(id: usize) -> &'static mut GasMixture {
		GAS_MIXTURES.read().unwrap().get_mut(id).unwrap()
	}
	pub fn register_gasmix(mix: &Value) {
		if NEXT_GAS_IDS.read().unwrap().is_empty() {
			let gas_mixtures = GAS_MIXTURES.write().unwrap();
			gas_mixtures.push(GasMixture::new());
			mix.set("_extools_pointer_gasmixture", g.len() - 1 as f32);
		} else {
			let next_gas_ids = NEXT_GAS_IDS.write().unwrap();
			let idx = next_gas_ids.pop().unwrap();
			let gas_mixtures = GAS_MIXTURES.write().unwrap();
			let gas_replacing = gas_mixtures.get_mut(idx).unwrap();
			*gas_replacing = GasMixture::new();
			mix.set("_extools_pointer_gasmixture", idx as f32);
		}
	}
	pub fn unregister_gasmix(mix: &Value) {
		let idx = mix
			.get("_extools_pointer_gasmixture")
			.unwrap()
			.as_number()
			.unwrap() as usize;
		if idx >= 0 {
			NEXT_GAS_IDS.write().unwrap().push(idx);
		}
		mix.set("_extools_pointer_gasmixture", &Value::null());
	}
}

pub fn get_mix(mix: &Value) -> &'static GasMixture {
	GasMixtures::get_gas_mix_by_id(mix.get_number("_extools_pointer_gasmixture").unwrap() as usize)
}
pub fn get_mix_mut(mix: &Value) -> &'static mut GasMixture {
	GasMixtures::get_gas_mix_by_id_mut(
		mix.get_number("_extools_pointer_gasmixture").unwrap() as usize
	)
}
