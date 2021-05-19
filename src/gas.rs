pub mod constants;
pub mod gas_mixture;
pub mod reaction;
use auxtools::*;

use std::collections::BTreeMap;

use gas_mixture::GasMixture;

use parking_lot::{const_rwlock, RwLock};

use std::sync::atomic::{AtomicU8, Ordering};

use std::cell::RefCell;

use reaction::Reaction;

static TOTAL_NUM_GASES: AtomicU8 = AtomicU8::new(0);

static GAS_SPECIFIC_HEAT: RwLock<Option<Vec<f32>>> = const_rwlock(None);

static GAS_VIS_THRESHOLD: RwLock<Option<Vec<Option<f32>>>> = const_rwlock(None); // the things we do for globals

static REACTION_INFO: RwLock<Option<Vec<Reaction>>> = const_rwlock(None);

thread_local! {
	static GAS_ID_TO_TYPE: RefCell<Vec<Value>> = RefCell::new(Vec::new());
	static GAS_ID_FROM_TYPE: RefCell<BTreeMap<u32,u8>> = RefCell::new(BTreeMap::new());
}

#[hook("/proc/auxtools_atmos_init")]
fn _hook_init() {
	*REACTION_INFO.write() = Some(get_reaction_info());
	let gas_types_list: auxtools::List = Proc::find("/proc/gas_types")
		.ok_or_else(|| runtime!("Could not find gas_types!"))?
		.call(&[])?
		.as_list()?;
	GAS_ID_TO_TYPE.with(|gt| {
		GAS_ID_FROM_TYPE.with(|gf| {
			let mut gas_id_to_type = gt.borrow_mut();
			let mut gas_id_from_type = gf.borrow_mut();
			gas_id_to_type.clear();
			gas_id_from_type.clear();
			let total_num_gases: u8 = gas_types_list.len() as u8;
			let mut gas_specific_heat: Vec<f32> = Vec::with_capacity(total_num_gases as usize);
			let mut gas_vis_threshold: Vec<Option<f32>> =
				Vec::with_capacity(total_num_gases as usize);
			let meta_gas_visibility_list: auxtools::List =
				Proc::find("/proc/meta_gas_visibility_list")
					.ok_or_else(|| runtime!("Invalid Could not find meta_gas_visibility_list!"))?
					.call(&[])?
					.as_list()?;
			for i in 1..gas_types_list.len() + 1 {
				let v = gas_types_list.get((i) as u32)?;
				gas_specific_heat.push(gas_types_list.get(&v)?.as_number()?);
				gas_vis_threshold.push(
					meta_gas_visibility_list
						.get(&v)
						.unwrap_or_else(|_| Value::null())
						.as_number()
						.ok(),
				);
				gas_id_from_type.insert(unsafe { v.value.data.id }, (i - 1) as u8);
				gas_id_to_type.push(v);
			}
			*GAS_SPECIFIC_HEAT.write() = Some(gas_specific_heat);
			*GAS_VIS_THRESHOLD.write() = Some(gas_vis_threshold);
			TOTAL_NUM_GASES.store(total_num_gases, Ordering::Release);
			Ok(Value::from(true))
		})
	})
}

fn get_reaction_info() -> Vec<Reaction> {
	let gas_reactions = Value::globals()
		.get("SSair")
		.unwrap()
		.get_list("gas_reactions")
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
	*REACTION_INFO.write() = Some(get_reaction_info());
	Ok(Value::from(true))
}

pub fn with_reactions<T, F>(mut f: F) -> T
where
	F: FnMut(&Vec<Reaction>) -> T,
{
	f(&REACTION_INFO
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Reactions not loaded yet! Uh oh!")))
}

/// Returns a static reference to a vector of all the specific heats of the gases.
pub fn gas_specific_heat(idx: u8) -> f32 {
	GAS_SPECIFIC_HEAT
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Specific heats not loaded yet! Uh oh!"))
		.get(idx as usize)
		.unwrap()
		.clone()
}

/// Returns the total number of gases in use. Only used by gas mixtures; should probably stay that way.
pub fn total_num_gases() -> u8 {
	TOTAL_NUM_GASES.load(Ordering::Relaxed)
}

/// Gets the gas visibility threshold for the given gas ID.
pub fn gas_visibility(idx: usize) -> Option<f32> {
	GAS_VIS_THRESHOLD
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gas visibility not loaded yet! Uh oh!"))
		.get(idx)
		.unwrap()
		.clone()
}

/// Returns the appropriate index to be used by the game for a given gas datum.
pub fn gas_id_from_type(path: &Value) -> Result<u8, Runtime> {
	GAS_ID_FROM_TYPE.with(|g| {
		Ok(*g
			.borrow()
			.get(&unsafe { path.value.data.id })
			.ok_or_else(|| runtime!("Invalid type! This should be a gas datum typepath!"))?)
	})
}

/// Takes an index and returns a Value representing the datum typepath of gas datum stored in that index.
pub fn gas_id_to_type(id: u8) -> DMResult {
	GAS_ID_TO_TYPE.with(|g| {
		let gas_id_to_type = g.borrow();
		Ok(gas_id_to_type
			.get(id as usize)
			.ok_or_else(|| runtime!("Invalid gas ID: {}", id))?
			.clone())
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
	static ref GAS_MIXTURES: RwLock<Vec<RwLock<GasMixture>>> =
		RwLock::new(Vec::with_capacity(100000));
}
thread_local! {
	static NEXT_GAS_IDS: RefCell<Vec<usize>> = RefCell::new(Vec::new());
}

impl GasMixtures {
	pub fn with_all_mixtures<F>(mut f: F)
	where
		F: FnMut(&Vec<RwLock<GasMixture>>),
	{
		f(&GAS_MIXTURES.read());
	}
	fn with_gas_mixture<T, F>(id: f32, mut f: F) -> Result<T, Runtime>
	where
		F: FnMut(&GasMixture) -> Result<T, Runtime>,
	{
		let mixtures = GAS_MIXTURES.read();
		let mix = mixtures
			.get(id.to_bits() as usize)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", id.to_bits()))?
			.read();
		f(&mix)
	}
	fn with_gas_mixture_mut<F>(id: f32, mut f: F) -> DMResult
	where
		F: FnMut(&mut GasMixture) -> DMResult,
	{
		let gas_mixtures = GAS_MIXTURES.read();
		let mut mix = gas_mixtures
			.get(id.to_bits() as usize)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", id.to_bits()))?
			.write();
		f(&mut mix)
	}
	fn with_gas_mixtures<F>(src: f32, arg: f32, mut f: F) -> DMResult
	where
		F: FnMut(&GasMixture, &GasMixture) -> DMResult,
	{
		let gas_mixtures = GAS_MIXTURES.read();
		let src_gas = gas_mixtures
			.get(src.to_bits() as usize)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src.to_bits()))?
			.read();
		let arg_gas = gas_mixtures
			.get(arg.to_bits() as usize)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", arg.to_bits()))?
			.read();
		f(&src_gas, &arg_gas)
	}
	fn with_gas_mixtures_mut<F>(src: f32, arg: f32, mut f: F) -> DMResult
	where
		F: FnMut(&mut GasMixture, &mut GasMixture) -> DMResult,
	{
		let src = src.to_bits() as usize;
		let arg = arg.to_bits() as usize;
		let gas_mixtures = GAS_MIXTURES.read();
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
	fn with_gas_mixtures_custom<F>(src: f32, arg: f32, mut f: F) -> DMResult
	where
		F: FnMut(&RwLock<GasMixture>, &RwLock<GasMixture>) -> DMResult,
	{
		let src = src.to_bits() as usize;
		let arg = arg.to_bits() as usize;
		let gas_mixtures = GAS_MIXTURES.read();
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
	/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument Value to point to it.
	pub fn register_gasmix(mix: &Value) -> DMResult {
		NEXT_GAS_IDS.with(|gas_ids| -> DMResult {
			if gas_ids.borrow().is_empty() {
				let mut gas_mixtures = GAS_MIXTURES.write();
				let next_idx = gas_mixtures.len();
				gas_mixtures.push(RwLock::new(GasMixture::from_vol(
					mix.get_number(byond_string!("initial_volume"))?,
				)));
				mix.set(
					byond_string!("_extools_pointer_gasmixture"),
					f32::from_bits(next_idx as u32),
				);
			} else {
				let idx = gas_ids.borrow_mut().pop().unwrap();
				GAS_MIXTURES
					.read()
					.get(idx)
					.unwrap()
					.write()
					.clear_with_vol(mix.get_number(byond_string!("initial_volume"))?);
				mix.set(
					byond_string!("_extools_pointer_gasmixture"),
					f32::from_bits(idx as u32),
				);
			}
			Ok(Value::null())
		})
	}
	/// Marks the Value's gas mixture as unused, allowing it to be reallocated to another.
	pub fn unregister_gasmix(mix: &Value) -> DMResult {
		if let Ok(float_bits) = mix.get_number(byond_string!("_extools_pointer_gasmixture")) {
			let idx = float_bits.to_bits();
			NEXT_GAS_IDS.with(|gas_ids| gas_ids.borrow_mut().push(idx as usize));
			mix.set(byond_string!("_extools_pointer_gasmixture"), &Value::null());
		}
		Ok(Value::null())
	}
}

#[shutdown]
fn _shut_down_heats()
// these are each called literally once per game so i can have as many as i want, neener neener
{
	let mut mixtures = GAS_MIXTURES.write();
	mixtures.clear();
}

/// Gets the mix for the given value, and calls the provided closure with a reference to that mix as an argument.
pub fn with_mix<T, F>(mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&GasMixture) -> Result<T, Runtime>,
{
	GasMixtures::with_gas_mixture(
		mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
		f,
	)
}

/// As with_mix, but mutable.
pub fn with_mix_mut<F>(mix: &Value, f: F) -> DMResult
where
	F: FnMut(&mut GasMixture) -> DMResult,
{
	GasMixtures::with_gas_mixture_mut(
		mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
		f,
	)
}

/// As with_mix, but with two mixes.
pub fn with_mixes<F>(src_mix: &Value, arg_mix: &Value, f: F) -> DMResult
where
	F: FnMut(&GasMixture, &GasMixture) -> DMResult,
{
	GasMixtures::with_gas_mixtures(
		src_mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
		arg_mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
		f,
	)
}

/// As with_mix_mut, but with two mixes.
pub fn with_mixes_mut<F>(src_mix: &Value, arg_mix: &Value, f: F) -> DMResult
where
	F: FnMut(&mut GasMixture, &mut GasMixture) -> DMResult,
{
	GasMixtures::with_gas_mixtures_mut(
		src_mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
		arg_mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
		f,
	)
}

/// Allows different lock levels for each gas. Instead of relevant refs to the gases, returns the RWLock object.
pub fn with_mixes_custom<F>(src_mix: &Value, arg_mix: &Value, f: F) -> DMResult
where
	F: FnMut(&RwLock<GasMixture>, &RwLock<GasMixture>) -> DMResult,
{
	GasMixtures::with_gas_mixtures_custom(
		src_mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
		arg_mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
		f,
	)
}

pub(crate) fn amt_gases() -> usize {
	NEXT_GAS_IDS.with(|next_gas_ids| GAS_MIXTURES.read().len() - next_gas_ids.borrow().len())
}

pub(crate) fn tot_gases() -> usize {
	GAS_MIXTURES.read().len()
}
