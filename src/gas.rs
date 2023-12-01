#[allow(dead_code)]
pub mod constants;

pub mod mixture;

pub mod types;

use byondapi::prelude::*;

pub use types::*;

use parking_lot::{const_rwlock, RwLock};

pub use mixture::Mixture;

pub type GasIDX = usize;

use eyre::Result;

/// A static container, with a bunch of helper functions for accessing global data. It's horrible, I know, but video games.
pub struct GasArena {}

/*
	This is where the gases live.
	This is just a big vector, acting as a gas mixture pool.
	As you can see, it can be accessed by any thread at any time;
	of course, it has a RwLock preventing this, and you can't access the
	vector directly. Seriously, please don't. I have the wrapper functions for a reason.
*/
static GAS_MIXTURES: RwLock<Option<Vec<RwLock<Mixture>>>> = const_rwlock(None);

static NEXT_GAS_IDS: RwLock<Option<Vec<usize>>> = const_rwlock(None);

pub fn initialize_gases() {
	*GAS_MIXTURES.write() = Some(Vec::with_capacity(240_000));
	*NEXT_GAS_IDS.write() = Some(Vec::with_capacity(2000));
}

pub fn shut_down_gases() {
	crate::turfs::wait_for_tasks();
	GAS_MIXTURES.write().as_mut().unwrap().clear();
	NEXT_GAS_IDS.write().as_mut().unwrap().clear();
}

impl GasArena {
	/// Locks the gas arena and and runs the given closure with it locked.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_all_mixtures<T, F>(f: F) -> T
	where
		F: FnOnce(&[RwLock<Mixture>]) -> T,
	{
		f(GAS_MIXTURES.read().as_ref().unwrap())
	}

	/// Locks the gas arena and and runs the given closure with it locked, fails if it can't acquire a lock in 30ms.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	#[allow(unused)]
	pub fn with_all_mixtures_fallible<T, F>(f: F) -> T
	where
		F: FnOnce(Option<&[RwLock<Mixture>]>) -> T,
	{
		let gases = GAS_MIXTURES.try_read_for(std::time::Duration::from_millis(30));
		f(gases.as_ref().unwrap().as_ref().map(|vec| vec.as_slice()))
	}
	/// Read locks the given gas mixture and runs the given closure on it.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_gas_mixture<T, F>(id: usize, f: F) -> Result<T>
	where
		F: FnOnce(&Mixture) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let mix = gas_mixtures
			.get(id)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", id))?
			.read();
		f(&mix)
	}
	/// Write locks the given gas mixture and runs the given closure on it.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_gas_mixture_mut<T, F>(id: usize, f: F) -> Result<T>
	where
		F: FnOnce(&mut Mixture) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let mut mix = gas_mixtures
			.get(id)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", id))?
			.write();
		f(&mut mix)
	}
	/// Read locks the given gas mixtures and runs the given closure on them.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_gas_mixtures<T, F>(src: usize, arg: usize, f: F) -> Result<T>
	where
		F: FnOnce(&Mixture, &Mixture) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let src_gas = gas_mixtures
			.get(src)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", src))?
			.read();
		let arg_gas = gas_mixtures
			.get(arg)
			.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", arg))?
			.read();
		f(&src_gas, &arg_gas)
	}
	/// Locks the given gas mixtures and runs the given closure on them.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	pub fn with_gas_mixtures_mut<T, F>(src: usize, arg: usize, f: F) -> Result<T>
	where
		F: FnOnce(&mut Mixture, &mut Mixture) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		if src == arg {
			let mut entry = gas_mixtures
				.get(src)
				.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", src))?
				.write();
			let mix = &mut entry;
			let mut copied = mix.clone();
			f(mix, &mut copied)
		} else {
			f(
				&mut gas_mixtures
					.get(src)
					.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", src))?
					.write(),
				&mut gas_mixtures
					.get(arg)
					.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", arg))?
					.write(),
			)
		}
	}
	/// Runs the given closure on the gas mixture *locks* rather than an already-locked version.
	/// # Errors
	/// If no such gas mixture exists or the closure itself errors.
	/// # Panics
	/// if `GAS_MIXTURES` hasn't been initialized, somehow.
	fn with_gas_mixtures_custom<T, F>(src: usize, arg: usize, f: F) -> Result<T>
	where
		F: FnOnce(&RwLock<Mixture>, &RwLock<Mixture>) -> Result<T>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		if src == arg {
			let entry = gas_mixtures
				.get(src)
				.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", src))?;
			let gas_copy = entry.read().clone();
			f(entry, &RwLock::new(gas_copy))
		} else {
			f(
				gas_mixtures
					.get(src)
					.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", src))?,
				gas_mixtures
					.get(arg)
					.ok_or_else(|| eyre::eyre!("No gas mixture with ID {} exists!", arg))?,
			)
		}
	}
	/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument ByondValue to point to it.
	/// # Errors
	/// If `initial_volume` is incorrect or `_extools_pointer_gasmixture` doesn't exist, somehow.
	/// # Panics
	/// If not called from the main thread
	/// If `NEXT_GAS_IDS` is not initialized, somehow.
	pub fn register_mix(mut mix: ByondValue) -> Result<ByondValue> {
		let init_volume = mix.read_number_id(byond_string!("initial_volume"))?;
		if NEXT_GAS_IDS.read().as_ref().unwrap().is_empty() {
			let mut gas_lock = GAS_MIXTURES.write();
			let gas_mixtures = gas_lock.as_mut().unwrap();
			let next_idx = gas_mixtures.len();
			gas_mixtures.push(RwLock::new(Mixture::from_vol(init_volume)));

			mix.write_var("_extools_pointer_gasmixture", &(next_idx as f32).into())
				.unwrap();

			let mut ids_lock = NEXT_GAS_IDS.write();
			let cur_last = gas_mixtures.len();
			let next_gas_ids = ids_lock.as_mut().unwrap();
			let cap = {
				let to_cap = gas_mixtures.capacity() - cur_last;
				if to_cap == 0 {
					next_gas_ids.capacity() - 100
				} else {
					(next_gas_ids.capacity() - 100).min(to_cap)
				}
			};
			next_gas_ids.extend(cur_last..(cur_last + cap));
			gas_mixtures.resize_with(cur_last + cap, Default::default);
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
				.clear_with_vol(init_volume);
			mix.write_var("_extools_pointer_gasmixture", &(idx as f32).into())
				.unwrap();
		}
		Ok(ByondValue::null())
	}
	/// Marks the ByondValue's gas mixture as unused, allowing it to be reallocated to another.
	/// # Panics
	/// If not called from the main thread
	/// If `NEXT_GAS_IDS` hasn't been initialized, somehow.
	pub fn unregister_mix(mix: &ByondValue) {
		if let Ok(idx) = mix.read_number_id(byond_string!("_extools_pointer_gasmixture")) {
			let mut next_gas_ids = NEXT_GAS_IDS.write();
			next_gas_ids.as_mut().unwrap().push(idx as usize);
		} else {
			panic!("Tried to unregister uninitialized mix")
		}
	}
}

/// Gets the mix for the given value, and calls the provided closure with a reference to that mix as an argument.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mix<T, F>(mix: &ByondValue, f: F) -> Result<T>
where
	F: FnOnce(&Mixture) -> Result<T>,
{
	GasArena::with_gas_mixture(
		mix.read_number_id(byond_string!("_extools_pointer_gasmixture"))? as usize,
		f,
	)
}

/// As `with_mix`, but mutable.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mix_mut<T, F>(mix: &ByondValue, f: F) -> Result<T>
where
	F: FnOnce(&mut Mixture) -> Result<T>,
{
	GasArena::with_gas_mixture_mut(
		mix.read_number_id(byond_string!("_extools_pointer_gasmixture"))? as usize,
		f,
	)
}

/// As `with_mix`, but with two mixes.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mixes<T, F>(src_mix: &ByondValue, arg_mix: &ByondValue, f: F) -> Result<T>
where
	F: FnOnce(&Mixture, &Mixture) -> Result<T>,
{
	GasArena::with_gas_mixtures(
		src_mix.read_number_id(byond_string!("_extools_pointer_gasmixture"))? as usize,
		arg_mix.read_number_id(byond_string!("_extools_pointer_gasmixture"))? as usize,
		f,
	)
}

/// As `with_mix_mut`, but with two mixes.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mixes_mut<T, F>(src_mix: &ByondValue, arg_mix: &ByondValue, f: F) -> Result<T>
where
	F: FnOnce(&mut Mixture, &mut Mixture) -> Result<T>,
{
	GasArena::with_gas_mixtures_mut(
		src_mix.read_number_id(byond_string!("_extools_pointer_gasmixture"))? as usize,
		arg_mix.read_number_id(byond_string!("_extools_pointer_gasmixture"))? as usize,
		f,
	)
}

/// Allows different lock levels for each gas. Instead of relevant refs to the gases, returns the `RWLock` object.
/// # Errors
/// If a gasmixture ID is not a number or the callback returns an error.
pub fn with_mixes_custom<T, F>(src_mix: &ByondValue, arg_mix: &ByondValue, f: F) -> Result<T>
where
	F: FnMut(&RwLock<Mixture>, &RwLock<Mixture>) -> Result<T>,
{
	GasArena::with_gas_mixtures_custom(
		src_mix.read_number_id(byond_string!("_extools_pointer_gasmixture"))? as usize,
		arg_mix.read_number_id(byond_string!("_extools_pointer_gasmixture"))? as usize,
		f,
	)
}

/// Gets the amount of gases that are active in byond.
/// # Panics
/// if `GAS_MIXTURES` hasn't been initialized, somehow.
pub fn amt_gases() -> usize {
	GAS_MIXTURES.read().as_ref().unwrap().len() - NEXT_GAS_IDS.read().as_ref().unwrap().len()
}

/// Gets the amount of gases that are allocated, but not necessarily active in byond.
/// # Panics
/// if `GAS_MIXTURES` hasn't been initialized, somehow.
pub fn tot_gases() -> usize {
	GAS_MIXTURES.read().as_ref().unwrap().len()
}
