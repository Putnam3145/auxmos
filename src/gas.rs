pub mod constants;
pub mod gas_mixture;
pub mod gases;
pub mod reaction;
pub mod simd_vector;

#[cfg(feature = "reaction_hooks")]
pub mod reaction_hooks;

pub use gases::*;

pub use gas_mixture::*;

type GasIDX = usize;

use auxtools::*;

use parking_lot::{const_rwlock, RwLock};

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
	*GAS_MIXTURES.write() = Some(Vec::with_capacity(120000));
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
	pub fn try_push_with_lock(vol: f32, g: &Vec<RwLock<GasMixture>>) -> Option<usize> {
		NEXT_GAS_IDS
			.try_write()
			.and_then(|mut lock| lock.as_mut().unwrap().pop())
			.map(|idx| {
				g.get(idx).unwrap().write().clear_with_vol(vol);
				idx
			})
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
			if let Some(mut lock) = GAS_MIXTURES.try_write() {
				let g = lock.as_mut().unwrap();
				let idx = g.len();
				g.push(RwLock::new(GasMixture::from_vol(vol)));
				Some(idx)
			} else {
				None
			}
		} else {
			None
		}
	}
	pub fn with_new_mix_and_lock<T>(
		vol: f32,
		g: &Vec<RwLock<GasMixture>>,
		f: impl FnOnce(&mut GasMixture) -> T,
	) -> T {
		if let Some(idx) = Self::try_push_with_lock(vol, g) {
			let mut mix = g.get(idx).unwrap().write();
			let ret = f(&mut mix);
			{
				let mut next_gas_ids = NEXT_GAS_IDS.write();
				next_gas_ids.as_mut().unwrap().push(idx);
			}
			ret
		} else {
			f(&mut GasMixture::from_vol(vol))
		}
	}
	pub fn with_new_mix<T>(vol: f32, f: impl FnOnce(&mut GasMixture) -> T) -> T {
		if let Some(idx) = Self::try_push(vol) {
			let lock = GAS_MIXTURES.read();
			let g = lock.as_ref().unwrap();
			let mut mix = g.get(idx).unwrap().write();
			let ret = f(&mut mix);
			{
				let mut next_gas_ids = NEXT_GAS_IDS.write();
				next_gas_ids.as_mut().unwrap().push(idx);
			}
			ret
		} else {
			f(&mut GasMixture::from_vol(vol))
		}
	}
	/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument Value to point to it.
	pub fn register_gasmix(mix: &Value) -> DMResult {
		if NEXT_GAS_IDS.read().as_ref().unwrap().is_empty() {
			let mut lock = GAS_MIXTURES.write();
			let gas_mixtures = lock.as_mut().unwrap();
			let next_idx = gas_mixtures.len();
			gas_mixtures.push(RwLock::new(GasMixture::from_vol(
				mix.get_number(byond_string!("initial_volume"))
					.map_err(|_| {
						runtime!(
							"Attempt to interpret non-number value as number {} {}:{}",
							std::file!(),
							std::line!(),
							std::column!()
						)
					})?,
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
				.clear_with_vol(
					mix.get_number(byond_string!("initial_volume"))
						.map_err(|_| {
							runtime!(
								"Attempt to interpret non-number value as number {} {}:{}",
								std::file!(),
								std::line!(),
								std::column!()
							)
						})?,
				);
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
}

pub fn value_to_idx(mix: &Value) -> Result<usize, Runtime> {
	mix.get_number(byond_string!("_extools_pointer_gasmixture"))
		.map_err(|_| {
			runtime!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})
		.map(|n| n.to_bits() as usize)
}

/// Gets the mix for the given value, and calls the provided closure with a reference to that mix as an argument.
pub fn with_mix<T, F>(mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&GasMixture) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixture(value_to_idx(mix)?, f)
}

/// As with_mix, but mutable.
pub fn with_mix_mut<T, F>(mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&mut GasMixture) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixture_mut(value_to_idx(mix)?, f)
}

/// As with_mix, but with two mixes.
pub fn with_mixes<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&GasMixture, &GasMixture) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixtures(value_to_idx(src_mix)?, value_to_idx(arg_mix)?, f)
}

/// As with_mix_mut, but with two mixes.
pub fn with_mixes_mut<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&mut GasMixture, &mut GasMixture) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixtures_mut(value_to_idx(src_mix)?, value_to_idx(arg_mix)?, f)
}

/// Allows different lock levels for each gas. Instead of relevant refs to the gases, returns the RWLock object.
pub fn with_mixes_custom<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&RwLock<GasMixture>, &RwLock<GasMixture>) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixtures_custom(value_to_idx(src_mix)?, value_to_idx(arg_mix)?, f)
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
