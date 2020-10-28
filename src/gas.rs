pub mod constants;
pub mod gas_mixture;
pub mod reaction;
use dm::*;

use multi_mut::BTreeMapMultiMut;

use std::collections::HashMap;

use gas_mixture::GasMixture;

use std::sync::RwLock;

use std::cell::RefCell;

use reaction::Reaction;

struct Gases {
	pub gas_ids: HashMap<u32, usize>,
	pub gas_specific_heat: Vec<f32>,
	pub total_num_gases: usize,
}

thread_local! {
	static GAS_ID_TO_TYPE: RefCell<Vec<Value>> = RefCell::new(Vec::new());
}

#[cfg(not(test))]
lazy_static! {
	static ref GAS_INFO: Gases = {
		let gas_types_list: dm::List = Proc::find("/proc/gas_types")
			.unwrap()
			.call(&[])
			.unwrap()
			.as_list()
			.unwrap();
		let mut gas_ids: HashMap<u32, usize> = HashMap::new();
		let mut gas_specific_heat: Vec<f32> = Vec::new();
		let total_num_gases: usize = gas_types_list.len() as usize;
		for i in 0..total_num_gases {
			let v = gas_types_list.get(i as u32).unwrap();
			unsafe {
				gas_ids.insert(v.value.data.id, i);
			}
			gas_specific_heat.push(v.as_number().unwrap_or(20.0));
			GAS_ID_TO_TYPE.with(|g| g.borrow_mut().push(v));
		}
		Gases {
			gas_ids,
			gas_specific_heat,
			total_num_gases,
		}
	};
	static ref REACTION_INFO: Vec<Reaction> = {
		let gas_reactions = Value::globals()
			.get("SSair")
			.unwrap()
			.get_list("gas_reactions")
			.unwrap();
		let mut reaction_cache: Vec<Reaction> = Vec::with_capacity(gas_reactions.len() as usize);
		for i in 0..gas_reactions.len() {
			let reaction = &gas_reactions.get(i).unwrap();
			reaction_cache.push(Reaction::from_byond_reaction(&reaction));
		}
		reaction_cache
	};
}

#[cfg(test)]
lazy_static! {
	static ref GAS_INFO: Gases = {
		let mut gas_ids: HashMap<u32, usize> = HashMap::new();
		for i in 0..5 {
			gas_ids.insert(i, i as usize);
		}
		let mut gas_specific_heat: Vec<f32> = vec![20.0, 20.0, 30.0, 200.0, 5.0];
		let mut gas_id_to_type: Vec<Value> = vec![
			Value::null(),
			Value::null(),
			Value::null(),
			Value::null(),
			Value::null(),
		];
		let total_num_gases: usize = 5;
		Gases {
			gas_ids,
			gas_specific_heat,
			gas_id_to_type,
			total_num_gases,
		}
	};
	static ref REACTION_INFO: Vec<Reaction> = { Vec::new() };
}

pub fn reactions() -> &'static Vec<Reaction> {
	&REACTION_INFO
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
	GAS_ID_TO_TYPE.with(|g| {
		let gas_id_to_type = g.borrow();
		if gas_id_to_type.len() < id {
			Ok(gas_id_to_type[id].clone())
		} else {
			Err(runtime!(format!("Invalid gas ID: {}", id)))
		}
	})
}

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

use crate::atmos_grid::turf_gases;

impl GasMixtures {
	fn with_gas_mixture<F>(id: f32, mut f: F) -> Result<Value, Runtime>
	where
		F: FnMut(&GasMixture) -> Result<Value, Runtime>,
	{
		if id.is_sign_negative() {
			f(&turf_gases().read().unwrap().get(&(-id as u32)).unwrap().mix)
		} else {
			f(GAS_MIXTURES.read().unwrap().get(id as usize).unwrap())
		}
	}
	fn with_gas_mixture_mut<F>(id: f32, mut f: F) -> Result<Value, Runtime>
	where
		F: FnMut(&mut GasMixture) -> Result<Value, Runtime>,
	{
		if id.is_sign_negative() {
			f(&mut turf_gases()
				.write()
				.unwrap()
				.get_mut(&(-id as u32))
				.unwrap()
				.mix)
		} else {
			f(GAS_MIXTURES.write().unwrap().get_mut(id as usize).unwrap())
		}
	}
	fn with_gas_mixtures<F>(src: f32, arg: f32, mut f: F) -> Result<Value, Runtime>
	where
		F: FnMut(&GasMixture, &GasMixture) -> Result<Value, Runtime>,
	{
		if src.is_sign_negative() || arg.is_sign_negative() {
			if src.is_sign_positive() {
				f(
					GAS_MIXTURES.read().unwrap().get(src as usize).unwrap(),
					&turf_gases()
						.read()
						.unwrap()
						.get(&(-arg as u32))
						.unwrap()
						.mix,
				)
			} else if arg.is_sign_positive() {
				f(
					&turf_gases()
						.read()
						.unwrap()
						.get(&(-src as u32))
						.unwrap()
						.mix,
					GAS_MIXTURES.read().unwrap().get(arg as usize).unwrap(),
				)
			} else {
				let gas_mixtures = turf_gases().read().unwrap();
				f(
					&gas_mixtures.get(&(-src as u32)).unwrap().mix,
					&gas_mixtures.get(&(-arg as u32)).unwrap().mix,
				)
			}
		} else {
			let gas_mixtures = GAS_MIXTURES.read().unwrap();
			f(
				gas_mixtures.get(src as usize).unwrap(),
				gas_mixtures.get(arg as usize).unwrap(),
			)
		}
	}
	fn with_gas_mixtures_mut<F>(src: f32, arg: f32, mut f: F) -> Result<Value, Runtime>
	where
		F: FnMut(&mut GasMixture, &mut GasMixture) -> Result<Value, Runtime>,
	{
		if src.is_sign_negative() || arg.is_sign_negative() {
			if src.is_sign_positive() {
				f(
					GAS_MIXTURES.write().unwrap().get_mut(src as usize).unwrap(),
					&mut turf_gases()
						.write()
						.unwrap()
						.get_mut(&(-arg as u32))
						.unwrap()
						.mix,
				)
			} else if arg.is_sign_positive() {
				f(
					GAS_MIXTURES.write().unwrap().get_mut(src as usize).unwrap(),
					&mut turf_gases()
						.write()
						.unwrap()
						.get_mut(&(-arg as u32))
						.unwrap()
						.mix,
				)
			} else {
				let mut gas_lock = turf_gases().write().unwrap();
				let (src_turf, arg_turf) = gas_lock
					.get_pair_mut(&(-src as u32), &(-arg as u32))
					.unwrap();

				f(&mut src_turf.mix, &mut arg_turf.mix)
			}
		} else {
			let mut gas_mixtures = GAS_MIXTURES.write().unwrap();
			let src_mix: &mut GasMixture;
			let arg_mix: &mut GasMixture;
			let src = src as usize;
			let arg = arg as usize;
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
	}
	/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument Value to point to it.
	pub fn register_gasmix(mix: &Value) -> Result<Value, Runtime> {
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
		Ok(Value::null())
	}
	/// Marks the Value's gas mixture as unused, allowing it to be reallocated to another.
	pub fn unregister_gasmix(mix: &Value) -> Result<Value, Runtime> {
		let idx = mix.get_number("_extools_pointer_gasmixture")?;
		if idx >= 0.0 {
			NEXT_GAS_IDS.write().unwrap().push(idx as usize);
		}
		mix.set("_extools_pointer_gasmixture", &Value::null());
		Ok(Value::null())
	}
}

/// Gets the mix for the given value, and calls the provided closure with a reference to that mix as an argument.
pub fn with_mix<F>(mix: &Value, f: F) -> Result<Value, Runtime>
where
	F: FnMut(&GasMixture) -> Result<Value, Runtime>,
{
	GasMixtures::with_gas_mixture(mix.get_number("_extools_pointer_gasmixture")?, f)
}

/// As with_mix, but mutable.
pub fn with_mix_mut<F>(mix: &Value, f: F) -> Result<Value, Runtime>
where
	F: FnMut(&mut GasMixture) -> Result<Value, Runtime>,
{
	GasMixtures::with_gas_mixture_mut(mix.get_number("_extools_pointer_gasmixture")?, f)
}

/// As with_mix, but with two mixes.
pub fn with_mixes<F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<Value, Runtime>
where
	F: FnMut(&GasMixture, &GasMixture) -> Result<Value, Runtime>,
{
	GasMixtures::with_gas_mixtures(
		src_mix.get_number("_extools_pointer_gasmixture")?,
		arg_mix.get_number("_extools_pointer_gasmixture")?,
		f,
	)
}

/// As with_mix_mut, but with two mixes.
pub fn with_mixes_mut<F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<Value, Runtime>
where
	F: FnMut(&mut GasMixture, &mut GasMixture) -> Result<Value, Runtime>,
{
	GasMixtures::with_gas_mixtures_mut(
		src_mix.get_number("_extools_pointer_gasmixture")?,
		arg_mix.get_number("_extools_pointer_gasmixture")?,
		f,
	)
}

pub fn amt_non_turf_gases() -> usize {
	GAS_MIXTURES.read().unwrap().len() - NEXT_GAS_IDS.read().unwrap().len()
}

pub fn tot_non_turf_gases() -> usize {
	GAS_MIXTURES.read().unwrap().len()
}
