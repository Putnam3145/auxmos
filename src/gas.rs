pub mod constants;
pub mod gas_mixture;
use dm::*;
use std::collections::HashMap;

struct Gases {
	pub gas_ids: HashMap<u32, usize>,
	pub gas_specific_heat: Vec<f32>,
	pub gas_id_to_type: Vec<Value>,
	pub total_num_gases: usize,
}

lazy_static! {
	static ref GAS_INFO: Gases = {
		let gas_types_list: dm::List =
			Proc::find("/proc/gas_types").unwrap().call(&[]).unwrap().as_list().unwrap();
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

/// Returns the appropriate index to be used by the game for a given gas datum.
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

/// Takes an index and returns a Value representing the datum typepath of gas datum stored in that index.
pub fn gas_id_to_type(id: usize) -> Result<Value, Runtime> {
	if GAS_INFO.gas_id_to_type.len() < id {
		Ok(GAS_INFO.gas_id_to_type[id].clone())
	} else {
		Err(runtime!(format!("Invalid gas ID: {}", id)))
	}
}

use gas_mixture::GasMixture;

use std::sync::RwLock;

pub struct GasMixtures {}

/*
	This is where the gases live.
	This is just a big vector, acting as a gas mixture pool.
	As you can see, it can be accessed by any thread at any time;
	of course, it has a RwLock preventing this, and you can't access the
	vector directly. Seriously, please don't. I have the wrapper functions for a reason.
*/
lazy_static! {
	static ref GAS_MIXTURES: RwLock<Vec<GasMixture>> = RwLock::new(Vec::with_capacity(200000));
}
lazy_static! {
	static ref NEXT_GAS_IDS: RwLock<Vec<usize>> = RwLock::new(Vec::new());
}
impl GasMixtures {
	fn with_gas_mixture<F>(id: usize, f: F) -> Result<Value, Runtime>
	where
		F: Fn(&GasMixture) -> Result<Value, Runtime>,
	{
		f(GAS_MIXTURES.read().unwrap().get(id).unwrap())
	}
	fn with_gas_mixture_mut<F>(id: usize, f: F) -> Result<Value, Runtime>
	where
		F: Fn(&mut GasMixture) -> Result<Value, Runtime>,
	{
		f(GAS_MIXTURES.write().unwrap().get_mut(id).unwrap())
	}
	fn with_gas_mixtures<F>(src: usize, arg: usize, f: F) -> Result<Value, Runtime>
	where
		F: Fn(&GasMixture, &GasMixture) -> Result<Value, Runtime>,
	{
		let gas_mixtures = GAS_MIXTURES.read().unwrap();
		f(
			gas_mixtures.get(src).unwrap(),
			gas_mixtures.get(arg).unwrap(),
		)
	}
	fn with_gas_mixtures_mut<F>(src: usize, arg: usize, f: F) -> Result<Value, Runtime>
	where
		F: Fn(&mut GasMixture, &mut GasMixture) -> Result<Value, Runtime>,
	{
		let mut gas_mixtures = GAS_MIXTURES.write().unwrap();
		let mut src_mix: &mut GasMixture;
		let mut arg_mix: &mut GasMixture;
		if src > arg {
			let split_idx = arg + 1;
			let (left, right) = gas_mixtures.split_at_mut(split_idx);
			arg_mix = left.last_mut().unwrap();
			src_mix = right.get_mut(src - split_idx).unwrap();
		} else if src < arg {
			let split_idx = src + 1;
			let (left, right) = gas_mixtures.split_at_mut(split_idx);
			src_mix = left.last_mut().unwrap();
			arg_mix = right.get_mut(arg - split_idx).unwrap();
		} else {
			src_mix = gas_mixtures.get_mut(src).unwrap();
			return f(src_mix, &mut src_mix.clone());
		}
		f(src_mix, arg_mix)
	}
	/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument Value to point to it.
	pub fn register_gasmix(mix: &Value) {
		if NEXT_GAS_IDS.read().unwrap().is_empty() {
			let mut gas_mixtures = GAS_MIXTURES.write().unwrap();
			gas_mixtures.push(GasMixture::new());
			mix.set(
				"_extools_pointer_gasmixture",
				(gas_mixtures.len() - 1) as f32,
			);
		} else {
			let idx = NEXT_GAS_IDS.write().unwrap().pop().unwrap();
			*GAS_MIXTURES.write().unwrap().get_mut(idx).unwrap() = GasMixture::new();
			mix.set("_extools_pointer_gasmixture", idx as f32);
		}
	}
	/// Marks the Value's gas mixture as unused, allowing it to be reallocated to another.
	pub fn unregister_gasmix(mix: &Value) -> Result<bool, Runtime> {
		let idx = mix.get_number("_extools_pointer_gasmixture")?;
		if idx >= 0.0 {
			NEXT_GAS_IDS.write().unwrap().push(idx as usize);
		}
		mix.set("_extools_pointer_gasmixture", &Value::null());
		Ok(true)
	}
}

/// Gets the mix for the given value, and calls the provided closure with a reference to that mix as an argument.
pub fn with_mix<F>(mix: &Value, f: F) -> Result<Value, Runtime>
where
	F: Fn(&GasMixture) -> Result<Value, Runtime>,
{
	GasMixtures::with_gas_mixture(mix.get_number("_extools_pointer_gasmixture")? as usize, f)
}

/// As with_mix, but mutable.
pub fn with_mix_mut<F>(mix: &Value, f: F) -> Result<Value, Runtime>
where
	F: Fn(&mut GasMixture) -> Result<Value, Runtime>,
{
	GasMixtures::with_gas_mixture_mut(mix.get_number("_extools_pointer_gasmixture")? as usize, f)
}

/// As with_mix, but with two mixes.
pub fn with_mixes<F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<Value, Runtime>
where
	F: Fn(&GasMixture, &GasMixture) -> Result<Value, Runtime>,
{
	GasMixtures::with_gas_mixtures(
		src_mix.get_number("_extools_pointer_gasmixture")? as usize,
		arg_mix.get_number("_extools_pointer_gasmixture")? as usize,
		f,
	)
}

/// As with_mix_mut, but with two mixes.
pub fn with_mixes_mut<F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<Value, Runtime>
where
	F: Fn(&mut GasMixture, &mut GasMixture) -> Result<Value, Runtime>,
{
	GasMixtures::with_gas_mixtures_mut(
		src_mix.get_number("_extools_pointer_gasmixture")? as usize,
		arg_mix.get_number("_extools_pointer_gasmixture")? as usize,
		f,
	)
}
