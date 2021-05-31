pub mod constants;
pub mod gas_mixture;
pub mod reaction;

#[cfg(feature = "reaction_hooks")]
pub mod reaction_hooks;

use auxtools::*;

use std::collections::{BTreeMap, HashMap};

use gas_mixture::GasMixture;

use parking_lot::{const_rwlock, RwLock};

use std::sync::atomic::{AtomicUsize, Ordering};

use std::cell::RefCell;

use reaction::Reaction;

static TOTAL_NUM_GASES: AtomicUsize = AtomicUsize::new(0);

static GAS_SPECIFIC_HEAT: RwLock<Option<Vec<f32>>> = const_rwlock(None);

static GAS_VIS_THRESHOLD: RwLock<Option<Vec<Option<f32>>>> = const_rwlock(None); // the things we do for globals

static REACTION_INFO: RwLock<Option<Vec<Reaction>>> = const_rwlock(None);

#[derive(Default)]
struct GasIDInfo {
	id_to_type: Vec<Value>,
	id_from_type: BTreeMap<u32, usize>,
	id_from_string: HashMap<std::string::String, usize>,
}

thread_local! {
	static GAS_ID_INFO: RefCell<GasIDInfo> = RefCell::new(Default::default());
	#[cfg(feature = "reaction_hooks")]
	static FUSION_POWER: RefCell<Vec<f32>> = RefCell::new(Vec::new())
}

#[hook("/proc/auxtools_atmos_init")]
fn _hook_init() {
	*REACTION_INFO.write() = Some(get_reaction_info());
	let gas_types_list: auxtools::List = Proc::find("/proc/gas_types")
		.ok_or_else(|| runtime!("Could not find gas_types!"))?
		.call(&[])?
		.as_list()?;
	GAS_ID_INFO.with(|g_| {
		let mut gas_id_info = g_.borrow_mut();
		*gas_id_info = Default::default();
		let total_num_gases = gas_types_list.len() as usize;
		let mut gas_specific_heat: Vec<f32> = Vec::with_capacity(total_num_gases);
		let mut gas_vis_threshold: Vec<Option<f32>> = Vec::with_capacity(total_num_gases);
		#[cfg(feature = "reaction_hooks")]
		let mut gas_fusion_powers: Vec<f32> = Vec::with_capacity(total_num_gases);
		let meta_gas_visibility_list: auxtools::List = Proc::find("/proc/meta_gas_visibility_list")
			.ok_or_else(|| runtime!("Could not find meta_gas_visibility_list!"))?
			.call(&[])?
			.as_list()?;
		#[cfg(feature = "reaction_hooks")]
		let meta_fusion_powers_list: auxtools::List = Proc::find("/proc/meta_gas_fusion_list")
			.ok_or_else(|| runtime!("Could not find meta_gas_fusion_list!"))?
			.call(&[])?
			.as_list()?;
		for i in 1..gas_types_list.len() + 1 {
			let v = gas_types_list.get(i)?;
			gas_specific_heat.push(gas_types_list.get(&v)?.as_number()?);
			gas_vis_threshold.push(
				meta_gas_visibility_list
					.get(&v)
					.unwrap_or_else(|_| Value::null())
					.as_number()
					.ok(),
			);
			#[cfg(feature = "reaction_hooks")]
			gas_fusion_powers.push(
				meta_fusion_powers_list
					.get(&v)
					.unwrap()
					.as_number()
					.unwrap(),
			);
			gas_id_info
				.id_from_type
				.insert(unsafe { v.raw.data.id }, (i - 1) as usize);
			let gas_str = v.to_string()?;
			if let Some(stripped) = gas_str.strip_prefix("/datum/gas/") {
				gas_id_info
					.id_from_string
					.insert(stripped.to_string(), (i - 1) as usize);
			} else {
				gas_id_info.id_from_string.insert(gas_str, (i - 1) as usize);
			}

			gas_id_info.id_to_type.push(v);
		}
		*GAS_SPECIFIC_HEAT.write() = Some(gas_specific_heat);
		*GAS_VIS_THRESHOLD.write() = Some(gas_vis_threshold);
		#[cfg(feature = "reaction_hooks")]
		FUSION_POWER.with(|f| {
			*f.borrow_mut() = gas_fusion_powers;
		});
		TOTAL_NUM_GASES.store(total_num_gases, Ordering::Release);
		Ok(Value::from(true))
	})
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
pub fn gas_specific_heat(idx: usize) -> f32 {
	GAS_SPECIFIC_HEAT
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Specific heats not loaded yet! Uh oh!"))
		.get(idx as usize)
		.unwrap()
		.clone()
}

#[cfg(feature = "reaction_hooks")]
pub fn gas_fusion_power(idx: &usize) -> f32 {
	FUSION_POWER.with(|g| *g.borrow().get(*idx as usize).unwrap())
}

/// Returns the total number of gases in use. Only used by gas mixtures; should probably stay that way.
pub fn total_num_gases() -> usize {
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
pub fn gas_id_from_type(path: &Value) -> Result<usize, Runtime> {
	GAS_ID_INFO.with(|g| {
		Ok(*g
			.borrow()
			.id_from_type
			.get(&unsafe { path.raw.data.id })
			.ok_or_else(|| runtime!("Invalid type! This should be a gas datum typepath!"))?)
	})
}

/// Takes an index and returns a Value representing the datum typepath of gas datum stored in that index.
pub fn gas_id_to_type(id: usize) -> DMResult {
	GAS_ID_INFO.with(|g| {
		Ok(g.borrow()
			.id_to_type
			.get(id)
			.ok_or_else(|| runtime!("Invalid gas ID: {}", id))?
			.clone())
	})
}

pub fn gas_id_from_type_name(name: &str) -> Result<usize, Runtime> {
	GAS_ID_INFO.with(|g| {
		Ok(*g
			.borrow()
			.id_from_string
			.get(name)
			.ok_or_else(|| runtime!("Invalid gas name: {}", name))?)
	})
}

pub struct GasMixtures {}

use std::convert::From;

struct Index(AtomicUsize);

impl Index {
	fn get(&self) -> Option<usize> {
		let i = self.0.load(Ordering::Relaxed);
		if i.leading_ones() == 0 {
			Some(i)
		} else {
			None
		}
	}
	fn set(&self, i: usize) {
		self.0.store(i, Ordering::Relaxed);
	}
	fn copy(&self, i: &Index) {
		self.0.store(i.0.load(Ordering::Relaxed), Ordering::Relaxed)
	}
	fn invalidate(&self) {
		self.0.store(usize::MAX, Ordering::Relaxed);
	}
	fn invalid() -> Self {
		Self(AtomicUsize::new(usize::MAX))
	}
}

impl From<Index> for Option<usize> {
	fn from(idx: Index) -> Self {
		idx.get()
	}
}

/*
	Much like https://docs.rs/generational-arena/0.2.8/generational_arena/struct.Arena.html,
	but that has properties I don't really need here, so this is my own kinda version.
	please don't use this for anything else it's got a lot of real stupid crap
	specifically taking the VERY SPECIFIC facts about the architecture I'm
	working in into account
*/

use std::cell::UnsafeCell;

pub struct Arena<T: Default> {
	internal: UnsafeCell<Vec<Box<[(Index, T)]>>>,
	first_free_idx: Index,
	len: AtomicUsize,
}

unsafe impl<T: Default> Sync for Arena<T> {}

impl<T: Default> Arena<T> {
	const SLICE_SIZE: usize = 16384;
	pub fn new() -> Self {
		Arena {
			internal: UnsafeCell::new(Vec::with_capacity(512)),
			first_free_idx: Index::invalid(),
			len: AtomicUsize::new(0),
		}
	}
	unsafe fn get_internal(&self, idx: usize) -> Option<&(Index, T)> {
		if let Some(slice) = self
			.internal
			.get()
			.as_ref()
			.unwrap()
			.get(idx / Self::SLICE_SIZE)
		{
			Some(slice.get_unchecked(idx % Self::SLICE_SIZE))
		} else {
			None
		}
	}
	unsafe fn get_mut_internal(&self, idx: usize) -> Option<&mut (Index, T)> {
		let internal = &mut *self.internal.get();
		if let Some(slice) = internal.get_mut(idx / Self::SLICE_SIZE) {
			Some(slice.get_unchecked_mut(idx % Self::SLICE_SIZE))
		} else {
			None
		}
	}
	pub fn get(&self, idx: usize) -> Option<&T> {
		if let Some(e) = unsafe { self.get_internal(idx) } {
			match e.0.get() {
				None => Some(&e.1),
				Some(_) => None,
			}
		} else {
			None
		}
	}
	pub fn get_mut(&self, idx: usize) -> Option<&mut T> {
		if let Some(e) = unsafe { self.get_mut_internal(idx) } {
			match e.0.get() {
				None => Some(&mut e.1),
				Some(_) => None,
			}
		} else {
			None
		}
	}
	pub fn len(&self) -> usize {
		self.len.load(Ordering::Relaxed)
	}
	pub fn internal_len(&self) -> usize {
		unsafe { self.internal.get().as_ref() }.unwrap().len() * Self::SLICE_SIZE
	}
	fn full(&self) -> bool {
		let internal = unsafe { self.internal.get().as_ref() }.unwrap();
		internal.len() == internal.capacity()
	}
	unsafe fn extend_push(&self, f: impl FnOnce(&mut T)) -> usize {
		if self.full() {
			panic!("Arena's been filled! No good!")
		}
		let cur_max = self.internal_len();
		let new_slice = (0..Self::SLICE_SIZE)
			.map(|i| (Index(AtomicUsize::new(i + cur_max + 1)), Default::default()))
			.collect::<Vec<_>>()
			.into_boxed_slice();
		new_slice.get(new_slice.len() - 1).unwrap().0.invalidate();
		let mut_internal = &mut *self.internal.get();
		mut_internal.push(new_slice);
		self.first_free_idx.set(cur_max);
		self.try_push(f).unwrap()
	}
	pub fn try_push(&self, g: impl FnOnce(&mut T)) -> Option<usize> {
		if let Some(idx) = self.first_free_idx.get() {
			let entry = unsafe { self.get_mut_internal(idx).unwrap() };
			self.first_free_idx.copy(&entry.0);
			entry.0.invalidate();
			g(&mut entry.1);
			self.len.fetch_add(1, Ordering::Relaxed);
			Some(idx)
		} else {
			None
		}
	}
	pub fn push_with(&self, f: impl FnOnce(&mut T) + Copy) -> usize {
		self.try_push(f)
			.unwrap_or_else(|| unsafe { self.extend_push(f) })
	}
	pub fn remove(&self, idx: usize) {
		if let Some(entry) = unsafe { self.get_internal(idx) } {
			entry.0.copy(&self.first_free_idx);
			self.first_free_idx.set(idx);
			self.len.fetch_sub(1, Ordering::Relaxed);
		}
	}
	pub fn clear(&self) {
		for i in 0..self.internal_len() - 1 {
			unsafe { self.get_internal(i) }.unwrap().0.set(i + 1);
		}
		unsafe { self.get_internal(self.internal_len() - 1) }
			.unwrap()
			.0
			.invalidate();
		self.len.store(0, Ordering::Relaxed);
		self.first_free_idx.set(0);
	}
}

enum GasMixtureOp {
	One(
		usize,
		Box<dyn FnMut(&mut GasMixture) -> DMResult + Send + Sync + 'static>,
	),
	Two(
		usize,
		usize,
		Box<dyn FnMut(&mut GasMixture, &mut GasMixture) -> DMResult + Send + Sync + 'static>,
	),
}

/*
	This is where the gases live.
	This is just a big vector, acting as a gas mixture pool.
	As you can see, it can be accessed by any thread at any time;
	of course, it has a RwLock preventing this, and you can't access the
	vector directly. Seriously, please don't. I have the wrapper functions for a reason.
*/
lazy_static! {
	static ref GAS_MIXTURES: Arena<GasMixture> = Arena::new();
	static ref DEFERRED_OPS: (flume::Sender<GasMixtureOp>, flume::Receiver<GasMixtureOp>) =
		flume::unbounded();
}

impl GasMixtures {
	pub fn with_all_mixtures<T, F>(mut f: F) -> T
	where
		F: FnMut(&Arena<GasMixture>) -> T,
	{
		f(&GAS_MIXTURES)
	}
	fn with_gas_mixture<T, F>(id: f32, mut f: F) -> Result<T, Runtime>
	where
		F: FnMut(&GasMixture) -> Result<T, Runtime>,
	{
		let mix = GAS_MIXTURES
			.get(id.to_bits() as usize)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", id.to_bits()))?;
		f(&mix)
	}
	fn with_gas_mixture_mut<T, F>(id: f32, mut f: F) -> Result<T, Runtime>
	where
		F: FnMut(&mut GasMixture) -> Result<T, Runtime>,
	{
		let mut mix = GAS_MIXTURES
			.get_mut(id.to_bits() as usize)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", id.to_bits()))?;
		f(&mut mix)
	}
	fn with_gas_mixtures<T, F>(src: f32, arg: f32, mut f: F) -> Result<T, Runtime>
	where
		F: FnMut(&GasMixture, &GasMixture) -> Result<T, Runtime>,
	{
		let src_gas = GAS_MIXTURES
			.get(src.to_bits() as usize)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src.to_bits()))?;
		let arg_gas = GAS_MIXTURES
			.get(arg.to_bits() as usize)
			.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", arg.to_bits()))?;
		f(&src_gas, &arg_gas)
	}
	fn with_gas_mixtures_mut<T, F>(src: f32, arg: f32, mut f: F) -> Result<T, Runtime>
	where
		F: FnMut(&mut GasMixture, &mut GasMixture) -> Result<T, Runtime>,
	{
		let src = src.to_bits() as usize;
		let arg = arg.to_bits() as usize;
		if src == arg {
			let mut entry = GAS_MIXTURES
				.get_mut(src)
				.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src))?;
			let mix = &mut entry;
			let mut copied = mix.clone();
			f(mix, &mut copied)
		} else {
			f(
				GAS_MIXTURES
					.get_mut(src)
					.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", src))?,
				GAS_MIXTURES
					.get_mut(arg)
					.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", arg))?,
			)
		}
	}
	/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument Value to point to it.
	pub fn register_gasmix(mix: &Value) -> DMResult {
		let vol = mix.get_number(byond_string!("initial_volume"))?;
		mix.set(
			byond_string!("_extools_pointer_gasmixture"),
			f32::from_bits(GAS_MIXTURES.push_with(|mix| mix.clear_with_vol(vol)) as u32),
		)?;
		Ok(Value::null())
	}
	/// Marks the Value's gas mixture as unused, allowing it to be reallocated to another.
	pub fn unregister_gasmix(mix: &Value) -> DMResult {
		if let Ok(float_bits) = mix.get_number(byond_string!("_extools_pointer_gasmixture")) {
			let idx = float_bits.to_bits();
			GAS_MIXTURES.remove(idx as usize);
			mix.set(byond_string!("_extools_pointer_gasmixture"), &Value::null())?;
		}
		Ok(Value::null())
	}
	/// Goes through all the deferred turf operations. This should probably be done often.
	pub fn do_deferred() -> Result<(), Runtime> {
		for op in DEFERRED_OPS.1.try_iter() {
			match op {
				GasMixtureOp::One(id, mut f) => {
					f(GAS_MIXTURES
						.get_mut(id)
						.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", id))?)?;
				}
				GasMixtureOp::Two(a, b, mut f) => {
					f(
						GAS_MIXTURES
							.get_mut(a)
							.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", a))?,
						GAS_MIXTURES
							.get_mut(b)
							.ok_or_else(|| runtime!("No gas mixture with ID {} exists!", b))?,
					)?;
				}
			}
		}
		Ok(())
	}
}

#[shutdown]
fn _shut_down_gases() {
	GAS_MIXTURES.clear();
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
	F: FnMut(&mut GasMixture) -> DMResult + Send + Sync + 'static,
{
	if super::turfs::processing::thread_running() && mix.is_exact_type("/datum/gas_mixture/turf") {
		let _ = DEFERRED_OPS.0.send(GasMixtureOp::One(
			mix.get_number(byond_string!("_extools_pointer_gasmixture"))?
				.to_bits() as usize,
			Box::new(f),
		));
		Ok(Value::null())
	} else {
		GasMixtures::with_gas_mixture_mut(
			mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
			f,
		)
	}
}

/// As with_mix, but with two mixes.
pub fn with_mixes<T, F>(src_mix: &Value, arg_mix: &Value, f: F) -> Result<T, Runtime>
where
	F: FnMut(&GasMixture, &GasMixture) -> Result<T, Runtime>,
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
	F: FnMut(&mut GasMixture, &mut GasMixture) -> DMResult + Send + Sync + 'static,
{
	if super::turfs::processing::thread_running()
		&& (src_mix.is_exact_type("/datum/gas_mixture/turf")
			|| arg_mix.is_exact_type("/datum/gas_mixture/turf"))
	{
		let _ = DEFERRED_OPS.0.send(GasMixtureOp::Two(
			src_mix
				.get_number(byond_string!("_extools_pointer_gasmixture"))?
				.to_bits() as usize,
			arg_mix
				.get_number(byond_string!("_extools_pointer_gasmixture"))?
				.to_bits() as usize,
			Box::new(f),
		));
		Ok(Value::null())
	} else {
		GasMixtures::with_gas_mixtures_mut(
			src_mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
			arg_mix.get_number(byond_string!("_extools_pointer_gasmixture"))?,
			f,
		)
	}
}

pub(crate) fn amt_gases() -> usize {
	GAS_MIXTURES.len()
}

pub(crate) fn tot_gases() -> usize {
	GAS_MIXTURES.internal_len()
}
