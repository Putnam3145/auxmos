pub mod constants;
pub mod gas_mixture;
pub mod reaction;

#[cfg(feature = "reaction_hooks")]
pub mod reaction_hooks;

use auxtools::*;

use std::collections::HashMap;

use gas_mixture::GasMixture;

use parking_lot::{const_rwlock, RwLock};

use std::sync::atomic::{AtomicU8, Ordering};

use fxhash::FxBuildHasher;

use std::cell::RefCell;

use reaction::Reaction;

static TOTAL_NUM_GASES: AtomicU8 = AtomicU8::new(0);

static mut REACTION_INFO: Option<Vec<Reaction>> = None;

#[derive(Clone)]
struct GasType {
	idx: u8,
	id: Box<str>,
	name: Box<str>,
	flags: u32,
	specific_heat: f32,
	fusion_power: f32,
	moles_visible: Option<f32>,
}

impl GasType {
	fn new(gas: &Value, idx: u8) -> Result<Self, Runtime> {
		Ok(Self {
			idx,
			id: gas.get_string(byond_string!("id"))?.into_boxed_str(),
			name: gas.get_string(byond_string!("name"))?.into_boxed_str(),
			flags: gas.get_number(byond_string!("flags"))? as u32,
			specific_heat: gas.get_number(byond_string!("specific_heat"))?,
			fusion_power: gas.get_number(byond_string!("fusion_power"))?,
			moles_visible: gas.get_number(byond_string!("moles_visible")).ok(),
		})
	}
}

static mut GAS_INFO_BY_STRING: Option<HashMap<String, GasType, FxBuildHasher>> = None;

static mut GAS_INFO_BY_IDX: Option<Vec<GasType>> = None;

#[hook("/proc/auxtools_atmos_init")]
fn _hook_init() {
	let gases = Value::globals().get_list(byond_string!("gas_data"))?;
	let total_num_gases: u8 = gases.len() as u8;
	let mut gas_table = HashMap::with_hasher(FxBuildHasher::default());
	let mut gas_list = Vec::with_capacity(total_num_gases as usize);
	for i in 0..gases.len() {
		let gas = gases.get(gases.get((i + 1) as u32)?)?;
		let gas_id = gas.get_string(byond_string!("id")).unwrap();
		let gas_cache = GasType::new(&gas, (i) as u8)?;
		gas_table.insert(gas_id, gas_cache.clone());
		gas_list.push(gas_cache);
	}
	unsafe {
		GAS_INFO_BY_STRING = Some(gas_table);
		GAS_INFO_BY_IDX = Some(gas_list);
		REACTION_INFO = Some(get_reaction_info())
	};
	TOTAL_NUM_GASES.store(total_num_gases, Ordering::Release);
	Ok(Value::from(true))
}

fn get_reaction_info() -> Vec<Reaction> {
	let gas_reactions = Value::globals()
		.get(byond_string!("SSair"))
		.unwrap()
		.get_list(byond_string!("gas_reactions"))
		.unwrap();
	let mut reaction_cache: Vec<Reaction> = Vec::with_capacity(gas_reactions.len() as usize);
	for i in 1..gas_reactions.len() + 1 {
		let reaction = &gas_reactions.get(i).unwrap();
		reaction_cache.push(Reaction::from_byond_reaction(&reaction));
	}
	reaction_cache
}

#[hook("/datum/controller/subsystem/air/proc/auxtools_update_reactions")]
fn _update_reactions() {
	unsafe { REACTION_INFO = Some(get_reaction_info()) };
	Ok(Value::from(true))
}

pub fn with_reactions<T, F>(mut f: F) -> T
where
	F: FnMut(&Vec<Reaction>) -> T,
{
	f(unsafe { REACTION_INFO.as_ref() }
		.unwrap_or_else(|| panic!("Reactions not loaded yet! Uh oh!")))
}

/// Returns a static reference to a vector of all the specific heats of the gases.
pub fn gas_specific_heat(idx: u8) -> f32 {
	unsafe { GAS_INFO_BY_IDX.as_ref() }
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.get(idx as usize)
		.unwrap()
		.specific_heat
}

#[cfg(feature = "reaction_hooks")]
pub fn gas_fusion_power(idx: &u8) -> f32 {
	unsafe { GAS_INFO_BY_IDX.as_ref() }
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.get(idx as usize)
		.unwrap()
		.fusion_power
}

/// Returns the total number of gases in use. Only used by gas mixtures; should probably stay that way.
pub fn total_num_gases() -> u8 {
	TOTAL_NUM_GASES.load(Ordering::Relaxed)
}

/// Gets the gas visibility threshold for the given gas ID.
pub fn gas_visibility(idx: usize) -> Option<f32> {
	unsafe { GAS_INFO_BY_IDX.as_ref() }
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.get(idx as usize)
		.unwrap()
		.moles_visible
}

thread_local! {
	static CACHED_GAS_IDS: RefCell<HashMap<Value, u8, FxBuildHasher>> = RefCell::new(HashMap::with_hasher(FxBuildHasher::default()));
}

/// Returns the appropriate index to be used by the game for a given ID string.
pub fn gas_idx_from_value(string_val: &Value) -> Result<u8, Runtime> {
	CACHED_GAS_IDS.with(|c| {
		let mut cache = c.borrow_mut();
		if let Some(idx) = cache.get(string_val) {
			Ok(*idx)
		} else {
			let id = &string_val.as_string()?;
			let idx = unsafe { GAS_INFO_BY_STRING.as_ref() }
				.ok_or_else(|| runtime!("Gases not loaded yet! Uh oh!"))?
				.get(id)
				.ok_or_else(|| runtime!("Invalid gas ID: {}", id))?
				.idx
				.clone();
			cache.insert(string_val.clone(), idx);
			Ok(idx)
		}
	})
}

/// Takes an index and returns a borrowed string representing the string ID of the gas datum stored in that index.
pub fn gas_idx_to_id(idx: u8) -> Result<&'static str, Runtime> {
	Ok(&unsafe { GAS_INFO_BY_IDX.as_ref() }
		.ok_or_else(|| runtime!("Gases not loaded yet! Uh oh!"))?
		.get(idx as usize)
		.ok_or_else(|| runtime!("Invalid gas index: {}", idx))?
		.id)
}

pub struct GasMixtures {}

/*
	This is where the gases live.
	This is just a big vector, acting as a gas mixture pool.
	As you can see, it can be accessed by any thread at any time;
	of course, it has a RwLock preventing this, and you can't access the
	vector directly. Seriously, please don't. I have the wrapper functions for a reason.
*/
static GAS_MIXTURES: RwLock<Option<Vec<RwLock<GasMixture>>>> = const_rwlock(None);

static NEXT_GAS_IDS: RwLock<Option<Vec<usize>>> = const_rwlock(None);

#[init(partial)]
fn _init_gas_mixtures() -> Result<(), String> {
	*GAS_MIXTURES.write() = Some(Vec::with_capacity(100000));
	*NEXT_GAS_IDS.write() = Some(Vec::with_capacity(2000));
	Ok(())
}

impl GasMixtures {
	pub fn with_all_mixtures<T, F>(f: F) -> T
	where
		F: FnOnce(&Vec<RwLock<GasMixture>>) -> T,
	{
		f(&GAS_MIXTURES.read().as_ref().unwrap())
	}
	fn with_gas_mixture<T, F>(id: usize, f: F) -> Result<T, Runtime>
	where
		F: FnOnce(&GasMixture) -> Result<T, Runtime>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let mix = gas_mixtures
			.get(id)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", id))?
			.read();
		f(&mix)
	}
	fn with_gas_mixture_mut<T, F>(id: usize, f: F) -> Result<T, Runtime>
	where
		F: FnOnce(&mut GasMixture) -> Result<T, Runtime>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let mut mix = gas_mixtures
			.get(id)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", id))?
			.write();
		f(&mut mix)
	}
	fn with_gas_mixtures<T, F>(src: usize, arg: usize, f: F) -> Result<T, Runtime>
	where
		F: FnOnce(&GasMixture, &GasMixture) -> Result<T, Runtime>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let src_gas = gas_mixtures
			.get(src)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src))?
			.read();
		let arg_gas = gas_mixtures
			.get(arg)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", arg))?
			.read();
		f(&src_gas, &arg_gas)
	}
	fn with_gas_mixtures_mut<T, F>(src: usize, arg: usize, f: F) -> Result<T, Runtime>
	where
		F: FnOnce(&mut GasMixture, &mut GasMixture) -> Result<T, Runtime>,
	{
		let src = src;
		let arg = arg;
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		if src == arg {
			let mut entry = gas_mixtures
				.get(src)
				.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src))?
				.write();
			let mix = &mut entry;
			let mut copied = mix.clone();
			f(mix, &mut copied)
		} else {
			f(
				&mut gas_mixtures
					.get(src)
					.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src))?
					.write(),
				&mut gas_mixtures
					.get(arg)
					.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", arg))?
					.write(),
			)
		}
	}
	fn with_gas_mixtures_custom<T, F>(src: usize, arg: usize, f: F) -> Result<T, Runtime>
	where
		F: FnOnce(&RwLock<GasMixture>, &RwLock<GasMixture>) -> Result<T, Runtime>,
	{
		let src = src;
		let arg = arg;
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		if src == arg {
			let entry = gas_mixtures
				.get(src)
				.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src))?;
			f(entry, entry.clone())
		} else {
			f(
				gas_mixtures
					.get(src)
					.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src))?,
				gas_mixtures
					.get(arg)
					.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", arg))?,
			)
		}
	}
	/// Makes a new living gas mixture and returns the index to it. Returns None if an allocation must happen.
	pub fn try_push(vol: f32) -> Option<usize> {
		if let Some(idx) = {
			let mut next_gas_ids = NEXT_GAS_IDS.write();
			next_gas_ids.as_mut().unwrap().pop()
		} {
			GAS_MIXTURES
				.read()
				.as_ref()
				.unwrap()
				.get(idx)
				.unwrap()
				.write()
				.clear_with_vol(vol);
			Some(idx)
		} else if {
			let lock = GAS_MIXTURES.read();
			let g = lock.as_ref().unwrap();
			g.len() == g.capacity()
		} {
			let mut lock = GAS_MIXTURES.write();
			let g = lock.as_mut().unwrap();
			let idx = g.len();
			g.push(RwLock::new(GasMixture::from_vol(vol)));
			Some(idx)
		} else {
			None
		}
	}
	/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument Value to point to it.
	pub fn register_gasmix(mix: &Value) -> DMResult {
		if NEXT_GAS_IDS.read().as_ref().unwrap().is_empty() {
			let mut lock = GAS_MIXTURES.write();
			let gas_mixtures = lock.as_mut().unwrap();
			let next_idx = gas_mixtures.len();
			gas_mixtures.push(RwLock::new(GasMixture::from_vol(
				mix.get_number(byond_string!("initial_volume"))?,
			)));
			mix.set(
				byond_string!("_extools_pointer_gasmixture"),
				f32::from_bits(next_idx as u32),
			)?;
		} else {
			let idx = {
				let mut next_gas_ids = NEXT_GAS_IDS.write();
				next_gas_ids.as_mut().unwrap().pop().unwrap()
			};
			GAS_MIXTURES
				.read()
				.as_ref()
				.unwrap()
				.get(idx)
				.unwrap()
				.write()
				.clear_with_vol(mix.get_number(byond_string!("initial_volume"))?);
			mix.set(
				byond_string!("_extools_pointer_gasmixture"),
				f32::from_bits(idx as u32),
			)?;
		}
		Ok(Value::null())
	}
	/// Marks the Value's gas mixture as unused, allowing it to be reallocated to another.
	pub fn unregister_gasmix(mix: &Value) -> DMResult {
		if let Ok(float_bits) = mix.get_number(byond_string!("_extools_pointer_gasmixture")) {
			let idx = float_bits.to_bits();
			{
				let mut next_gas_ids = NEXT_GAS_IDS.write();
				next_gas_ids.as_mut().unwrap().push(idx as usize);
			}
			mix.set(byond_string!("_extools_pointer_gasmixture"), &Value::null())?;
		}
		Ok(Value::null())
	}
}

#[shutdown]
fn _shut_down_gases() {
	*GAS_MIXTURES.write() = None;
	*NEXT_GAS_IDS.write() = None;
	unsafe {
		GAS_INFO_BY_IDX = None;
		GAS_INFO_BY_STRING = None;
	}
	CACHED_GAS_IDS.with(|gas_ids| {
		gas_ids.borrow_mut().clear();
	});
}

/// Gets the mix for the given value, and calls the provided closure with a reference to that mix as an argument.
pub fn with_mix<T, F>(mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&GasMixture) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixture(
		mix.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize,
		f,
	)
}

/// As with_mix, but mutable.
pub fn with_mix_mut<T, F>(mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&mut GasMixture) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixture_mut(
		mix.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize,
		f,
	)
}

/// As with_mix, but with two mixes.
pub fn with_mixes<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&GasMixture, &GasMixture) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixtures(
		src_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize,
		arg_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize,
		f,
	)
}

/// As with_mix_mut, but with two mixes.
pub fn with_mixes_mut<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&mut GasMixture, &mut GasMixture) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixtures_mut(
		src_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize,
		arg_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize,
		f,
	)
}

/// Allows different lock levels for each gas. Instead of relevant refs to the gases, returns the RWLock object.
pub fn with_mixes_custom<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&RwLock<GasMixture>, &RwLock<GasMixture>) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixtures_custom(
		src_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize,
		arg_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize,
		f,
	)
}

#[hook("/proc/fix_corrupted_atmos")]
fn _fix_corrupted_atmos() {
	rayon::spawn(|| {
		for lock in GAS_MIXTURES.read().as_ref().unwrap().iter() {
			if {
				if let Some(gas) = lock.try_read() {
					gas.is_corrupt()
				} else {
					false
				}
			} {
				lock.write().fix_corruption();
			}
		}
	});
	Ok(Value::null())
}

pub(crate) fn amt_gases() -> usize {
	GAS_MIXTURES.read().as_ref().unwrap().len() - NEXT_GAS_IDS.read().as_ref().unwrap().len()
}

pub(crate) fn tot_gases() -> usize {
	GAS_MIXTURES.read().as_ref().unwrap().len()
}
