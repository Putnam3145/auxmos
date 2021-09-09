pub mod constants;
pub mod mixture;
pub mod types;

use auxtools::*;

use fxhash::FxBuildHasher;

use std::collections::HashSet;

use parking_lot::{const_rwlock, RwLock};

pub use types::*;

pub use mixture::Mixture;

pub type GasIDX = usize;

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

static mut REGISTERED_GAS_MIXES: Option<HashSet<u32, FxBuildHasher>> = None;

fn is_registered_mix(i: u32) -> bool {
	unsafe {
		REGISTERED_GAS_MIXES
			.as_ref()
			.map_or(false, |map| map.contains(&i))
	}
}

fn register_mix(v: &Value) {
	unsafe {
		REGISTERED_GAS_MIXES
			.get_or_insert_with(|| HashSet::with_hasher(FxBuildHasher::default()))
			.insert(v.raw.data.id);
	}
}

fn unregister_mix(i: u32) {
	unsafe {
		REGISTERED_GAS_MIXES
			.get_or_insert_with(|| HashSet::with_hasher(FxBuildHasher::default()))
			.remove(&i);
	}
}

#[init(partial)]
fn _init_gas_mixtures() -> Result<(), String> {
	*GAS_MIXTURES.write() = Some(Vec::with_capacity(240_000));
	*NEXT_GAS_IDS.write() = Some(Vec::with_capacity(2000));
	Ok(())
}

#[shutdown]
fn _shut_down_gases() {
	*GAS_MIXTURES.write() = None;
	*NEXT_GAS_IDS.write() = None;
	unsafe {
		REGISTERED_GAS_MIXES
			.get_or_insert_with(|| HashSet::with_hasher(FxBuildHasher::default()))
			.clear();
	}
}

impl GasArena {
	pub fn with_all_mixtures<T, F>(f: F) -> T
	where
		F: FnOnce(&[RwLock<Mixture>]) -> T,
	{
		f(GAS_MIXTURES.read().as_ref().unwrap())
	}
	pub fn with_gas_mixture<T, F>(id: usize, f: F) -> Result<T, Runtime>
	where
		F: FnOnce(&Mixture) -> Result<T, Runtime>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let mix = gas_mixtures
			.get(id)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", id))?
			.read();
		f(&mix)
	}
	pub fn with_gas_mixture_mut<T, F>(id: usize, f: F) -> Result<T, Runtime>
	where
		F: FnOnce(&mut Mixture) -> Result<T, Runtime>,
	{
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		let mut mix = gas_mixtures
			.get(id)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", id))?
			.write();
		f(&mut mix)
	}
	pub fn with_gas_mixtures<T, F>(src: usize, arg: usize, f: F) -> Result<T, Runtime>
	where
		F: FnOnce(&Mixture, &Mixture) -> Result<T, Runtime>,
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
	pub fn with_gas_mixtures_mut<T, F>(src: usize, arg: usize, f: F) -> Result<T, Runtime>
	where
		F: FnOnce(&mut Mixture, &mut Mixture) -> Result<T, Runtime>,
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
		F: FnOnce(&RwLock<Mixture>, &RwLock<Mixture>) -> Result<T, Runtime>,
	{
		let src = src;
		let arg = arg;
		let lock = GAS_MIXTURES.read();
		let gas_mixtures = lock.as_ref().unwrap();
		if src == arg {
			let entry = gas_mixtures
				.get(src)
				.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src))?;
			let gas_copy = entry.read().clone();
			f(entry, &RwLock::new(gas_copy))
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
	/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument Value to point to it.
	pub fn register_mix(mix: &Value) -> DMResult {
		if NEXT_GAS_IDS.read().as_ref().unwrap().is_empty() {
			let mut lock = GAS_MIXTURES.write();
			let gas_mixtures = lock.as_mut().unwrap();
			let next_idx = gas_mixtures.len();
			gas_mixtures.push(RwLock::new(Mixture::from_vol(
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
		register_mix(mix);
		Ok(Value::null())
	}
	/// Marks the Value's gas mixture as unused, allowing it to be reallocated to another.
	pub fn unregister_mix(mix: u32) {
		if is_registered_mix(mix) {
			use raw_types::values::{ValueData, ValueTag};
			unsafe {
				let mut raw = raw_types::values::Value {
					tag: ValueTag::Null,
					data: ValueData { id: 0 },
				};
				let this_mix = raw_types::values::Value {
					tag: ValueTag::Datum,
					data: ValueData { id: mix },
				};
				let err = raw_types::funcs::get_variable(
					&mut raw,
					this_mix,
					byond_string!("_extools_pointer_gasmixture").get_id(),
				);
				if err == 1 {
					let idx = raw.data.number.to_bits();
					{
						let mut next_gas_ids = NEXT_GAS_IDS.write();
						next_gas_ids.as_mut().unwrap().push(idx as usize);
					}
					unregister_mix(mix);
				}
			}
		}
	}
}

/// Gets the mix for the given value, and calls the provided closure with a reference to that mix as an argument.
pub fn with_mix<T, F>(mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&Mixture) -> Result<T, Runtime>,
{
	GasArena::with_gas_mixture(
		mix.get_number(byond_string!("_extools_pointer_gasmixture"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?
			.to_bits() as usize,
		f,
	)
}

/// As `with_mix`, but mutable.
pub fn with_mix_mut<T, F>(mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&mut Mixture) -> Result<T, Runtime>,
{
	GasArena::with_gas_mixture_mut(
		mix.get_number(byond_string!("_extools_pointer_gasmixture"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?
			.to_bits() as usize,
		f,
	)
}

/// As `with_mix`, but with two mixes.
pub fn with_mixes<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&Mixture, &Mixture) -> Result<T, Runtime>,
{
	GasArena::with_gas_mixtures(
		src_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?
			.to_bits() as usize,
		arg_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?
			.to_bits() as usize,
		f,
	)
}

/// As `with_mix_mut`, but with two mixes.
pub fn with_mixes_mut<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&mut Mixture, &mut Mixture) -> Result<T, Runtime>,
{
	GasArena::with_gas_mixtures_mut(
		src_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?
			.to_bits() as usize,
		arg_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?
			.to_bits() as usize,
		f,
	)
}

/// Allows different lock levels for each gas. Instead of relevant refs to the gases, returns the `RWLock` object.
pub fn with_mixes_custom<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&RwLock<Mixture>, &RwLock<Mixture>) -> Result<T, Runtime>,
{
	GasArena::with_gas_mixtures_custom(
		src_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?
			.to_bits() as usize,
		arg_mix
			.get_number(byond_string!("_extools_pointer_gasmixture"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?
			.to_bits() as usize,
		f,
	)
}

pub(crate) fn amt_gases() -> usize {
	GAS_MIXTURES.read().as_ref().unwrap().len() - NEXT_GAS_IDS.read().as_ref().unwrap().len()
}

pub(crate) fn tot_gases() -> usize {
	GAS_MIXTURES.read().as_ref().unwrap().len()
}
