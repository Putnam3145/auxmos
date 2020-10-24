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
	pub static ref STR_ID_EXTOOLS_POINTER: u32 = {
		let v = Value::from_string("_extools_pointer_gasmixture");
		unsafe {
			v.value.data.id
		}
	};
}

/// Returns a static reference to a vector of all the specific heats of the gases.
pub fn gas_specific_heats() -> &'static Vec<f32> {
	&GAS_INFO.gas_specific_heat
}

/// Returns the total number of gases in use. Only used by gas mixtures; should probably stay that way.
pub fn total_num_gases() -> u32 {
	GAS_INFO.total_num_gases
}

use gas_mixture::GasMixture;

pub struct GasMixtures {}

use std::cell::RefCell;

impl GasMixtures {
	thread_local! {
		static GAS_MIXTURES: Vec<RefCell<GasMixture>> = Vec::new(); // not with capacity 200000 because I'm only storing it on the one thread, then channeling it around
		static NEXT_GAS_IDS: Vec<usize> = Vec::new();
	}
	/// Returns a RefCell, not a mix directly; thing that called this can decide whether to borrow mutably or not.
	pub fn get_gas_mix(id : usize) -> &'static RefCell<GasMixture> {
		&GasMixtures::GAS_MIXTURES.with(|g| *g.get(id).unwrap())
	}
	pub fn register_gasmix(mix : &Value) {
		GasMixtures::NEXT_GAS_IDS.with(|next_gas_ids| {
			if next_gas_ids.is_empty() {
				GasMixtures::GAS_MIXTURES.with(|g| {
					*g.push(RefCell::new(GasMixture::new()));
					mix.set("_extools_pointer_gasmixture", g.len() as f32);
				});
			} else {
				let idx = next_gas_ids.pop().unwrap();
				GasMixtures::GAS_MIXTURES.with(|g| {
					let mut gas_replacing = *g.get(idx).unwrap().borrow_mut();
					gas_replacing = GasMixture::new();
					mix.set("_extools_pointer_gasmixture", g.len() as f32);
				});
			}
		})
	}
	pub fn unregister_gasmix(mix: &Value) {
		let idx = mix.get("_extools_pointer_gasmixture").unwrap().as_number().unwrap() as usize;
		if idx != 0 {
			GasMixtures::NEXT_GAS_IDS.with(|next_gas_ids| {
				next_gas_ids.push(idx);
			});
		}
		mix.set("_extools_pointer_gasmixture",&Value::null());
	}
}
