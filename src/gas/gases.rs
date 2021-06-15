use std::sync::atomic::{AtomicUsize, Ordering};

use fxhash::FxBuildHasher;

use std::cell::RefCell;

use super::{gas_mixture::FlatSimdVec, reaction::Reaction, GasIDX};

use dashmap::DashMap;

use std::collections::{HashMap, HashSet};

static TOTAL_NUM_GASES: AtomicUsize = AtomicUsize::new(0);

use auxtools::*;

use parking_lot::{const_rwlock, RwLock};
#[derive(Clone, Copy)]
pub struct OxidationInfo {
	temperature: f32,
	power: f32,
}

impl OxidationInfo {
	pub fn temperature(&self) -> f32 {
		self.temperature
	}
	pub fn power(&self) -> f32 {
		self.power
	}
}

#[derive(Clone, Copy)]
pub struct FireInfo {
	temperature: f32,
	burn_rate: f32,
}

impl FireInfo {
	pub fn temperature(&self) -> f32 {
		self.temperature
	}
	pub fn burn_rate(&self) -> f32 {
		self.burn_rate
	}
}

#[derive(Clone)]
pub enum GasRef {
	Deferred(String),
	Found(GasIDX),
}

impl GasRef {
	pub fn get(&self) -> Result<GasIDX, Runtime> {
		match self {
			Self::Deferred(s) => gas_idx_from_string(s),
			Self::Found(id) => Ok(*id),
		}
	}
	pub fn update(&mut self) -> Result<GasIDX, Runtime> {
		match self {
			Self::Deferred(s) => {
				*self = Self::Found(gas_idx_from_string(s)?);
				self.get()
			}
			Self::Found(id) => Ok(*id),
		}
	}
}

#[derive(Clone)]
pub struct GasType {
	pub idx: GasIDX,
	pub id: &'static str,
	pub name: Box<str>,
	pub flags: u32,
	pub specific_heat: f32,
	pub fusion_power: f32,
	pub moles_visible: Option<f32>,
	pub fire_energy_released: f32,
	pub oxidation: Option<OxidationInfo>,
	pub fire: Option<FireInfo>,
	pub fire_products: Option<Vec<(GasRef, f32)>>,
}

// don't want to clean this up, it's got actual leaked rust memory
static mut KNOWN_GASES: Option<HashSet<&'static str, FxBuildHasher>> = None;

impl GasType {
	fn new(gas: &Value, idx: GasIDX) -> Result<Self, Runtime> {
		Ok(Self {
			idx,
			id: {
				let s = gas.get_string(byond_string!("id"))?;
				super::constants::is_hardcoded_gas(&s).unwrap_or_else(|| {
					if let Some(r) = unsafe { KNOWN_GASES.as_ref() }.and_then(|k| k.get(&s as &str))
					{
						r
					} else {
						let r = Box::leak(s.into_boxed_str());
						unsafe {
							KNOWN_GASES.get_or_insert_with(|| {
								HashSet::with_hasher(FxBuildHasher::default())
							})
						}
						.insert(r);
						r
					}
				})
			},
			name: gas.get_string(byond_string!("name"))?.into_boxed_str(),
			flags: gas.get_number(byond_string!("flags")).map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})? as u32,
			specific_heat: gas
				.get_number(byond_string!("specific_heat"))
				.map_err(|_| {
					runtime!(
						"Attempt to interpret non-number value as number {} {}:{}",
						std::file!(),
						std::line!(),
						std::column!()
					)
				})?,
			fusion_power: gas.get_number(byond_string!("fusion_power")).map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?,
			moles_visible: gas.get_number(byond_string!("moles_visible")).ok(),
			oxidation: if let Ok(temperature) =
				gas.get_number(byond_string!("oxidation_temperature"))
			{
				Some(OxidationInfo {
					temperature,
					power: gas.get_number(byond_string!("oxidation_rate"))?,
				})
			} else {
				None
			},
			fire: if let Ok(temperature) = gas.get_number(byond_string!("fire_temperature")) {
				Some(FireInfo {
					temperature,
					burn_rate: gas.get_number(byond_string!("fire_burn_rate"))?,
				})
			} else {
				None
			},
			fire_products: if let Ok(products) = gas.get_list(byond_string!("fire_products")) {
				Some(
					(1..products.len() + 1)
						.map(|i| {
							let s = products.get(i).unwrap();
							(
								GasRef::Deferred(s.as_string().unwrap()),
								products.get(s).unwrap().as_number().unwrap(),
							)
						})
						.collect(),
				)
			} else {
				None
			},
			fire_energy_released: gas.get_number(byond_string!("fire_energy_released"))?,
		})
	}
}

static mut GAS_INFO_BY_STRING: Option<DashMap<Box<str>, GasType, FxBuildHasher>> = None;

static GAS_INFO_BY_IDX: RwLock<Option<Vec<GasType>>> = const_rwlock(None);

static SPECIFIC_HEATS: RwLock<Option<FlatSimdVec>> = const_rwlock(None);

#[cfg(feature = "reaction_hooks")]
pub(crate) static FUSION_POWERS: RwLock<Option<FlatSimdVec>> = const_rwlock(None);

// turns out AoS will always be the way to go forever, who knew

pub(crate) static OXIDATION_TEMPS: RwLock<Option<FlatSimdVec>> = const_rwlock(None);

pub(crate) static OXIDATION_POWERS: RwLock<Option<FlatSimdVec>> = const_rwlock(None);

pub(crate) static FIRE_TEMPS: RwLock<Option<FlatSimdVec>> = const_rwlock(None);

pub(crate) static FIRE_RATES: RwLock<Option<FlatSimdVec>> = const_rwlock(None);

pub(crate) static FIRE_ENTHALPIES: RwLock<Option<FlatSimdVec>> = const_rwlock(None);

pub(crate) static VISIBILITIES: RwLock<Option<FlatSimdVec>> = const_rwlock(None);

pub(crate) static FIRE_PRODUCTS: RwLock<Option<Vec<FlatSimdVec>>> = const_rwlock(None); // this one's lazy

#[init(partial)]
fn _create_gas_info_structs() -> Result<(), String> {
	unsafe { GAS_INFO_BY_STRING = Some(DashMap::with_hasher(FxBuildHasher::default())) };
	*GAS_INFO_BY_IDX.write() = Some(Vec::new());
	*SPECIFIC_HEATS.write() = Some(FlatSimdVec::new());
	#[cfg(feature = "reaction_hooks")]
	{
		*FUSION_POWERS.write() = Some(FlatSimdVec::new())
	}
	*OXIDATION_TEMPS.write() = Some(FlatSimdVec::new());
	*OXIDATION_POWERS.write() = Some(FlatSimdVec::new());
	*FIRE_TEMPS.write() = Some(FlatSimdVec::new());
	*FIRE_RATES.write() = Some(FlatSimdVec::new());
	*FIRE_ENTHALPIES.write() = Some(FlatSimdVec::new());
	*VISIBILITIES.write() = Some(FlatSimdVec::new());
	Ok(())
}

#[shutdown]
fn _destroy_gas_info_structs() {
	unsafe { GAS_INFO_BY_STRING = None };
	*GAS_INFO_BY_IDX.write() = None;
	*SPECIFIC_HEATS.write() = None;
	#[cfg(feature = "reaction_hooks")]
	{
		*FUSION_POWERS.write() = None;
	}
	*OXIDATION_TEMPS.write() = None;
	*OXIDATION_POWERS.write() = None;
	*FIRE_TEMPS.write() = None;
	*FIRE_RATES.write() = None;
	*FIRE_ENTHALPIES.write() = None;
	*VISIBILITIES.write() = None;
	TOTAL_NUM_GASES.store(0, Ordering::Release);
	CACHED_GAS_IDS.with(|gas_ids| {
		gas_ids.borrow_mut().clear();
	});
}

#[hook("/proc/_auxtools_register_gas")]
fn _hook_register_gas(gas: Value) {
	let gas_id = gas.get_string(byond_string!("id")).unwrap();
	let new_gas_id = TOTAL_NUM_GASES.fetch_add(1, Ordering::Release); // this is the only thing that stores it other than shutdown
	let gas_cache = GasType::new(&gas, new_gas_id)?;
	unsafe { GAS_INFO_BY_STRING.as_ref() }
		.unwrap()
		.insert(gas_id.into_boxed_str(), gas_cache.clone());
	SPECIFIC_HEATS
		.write()
		.as_mut()
		.unwrap()
		.push(gas_cache.specific_heat);
	#[cfg(feature = "reaction_hooks")]
	{
		FUSION_POWERS
			.write()
			.as_mut()
			.unwrap()
			.push(gas_cache.fusion_power);
	}
	if let Some(oxidation) = gas_cache.oxidation {
		OXIDATION_TEMPS
			.write()
			.as_mut()
			.unwrap()
			.push(oxidation.temperature());
		OXIDATION_POWERS
			.write()
			.as_mut()
			.unwrap()
			.push(oxidation.power());
	} else {
		OXIDATION_TEMPS.write().as_mut().unwrap().push(f32::MAX);
		OXIDATION_POWERS.write().as_mut().unwrap().push(0.0);
	}
	if let Some(fire) = gas_cache.fire {
		FIRE_TEMPS
			.write()
			.as_mut()
			.unwrap()
			.push(fire.temperature());
		FIRE_RATES.write().as_mut().unwrap().push(fire.burn_rate());
	} else {
		FIRE_TEMPS.write().as_mut().unwrap().push(f32::MAX);
		FIRE_RATES.write().as_mut().unwrap().push(0.0);
	}
	VISIBILITIES
		.write()
		.as_mut()
		.unwrap()
		.push(gas_cache.moles_visible.unwrap_or(f32::MAX));
	FIRE_ENTHALPIES
		.write()
		.as_mut()
		.unwrap()
		.push(gas_cache.fire_energy_released);
	GAS_INFO_BY_IDX.write().as_mut().unwrap().push(gas_cache);
	*FIRE_PRODUCTS.write() = None;
	Ok(Value::null())
}

static mut REACTION_INFO: Option<Vec<Reaction>> = None;

#[hook("/proc/auxtools_atmos_init")]
fn _hook_init() {
	let data = Value::globals()
		.get(byond_string!("gas_data"))?
		.get_list(byond_string!("datums"))?;
	for i in 1..=data.len() {
		_hook_register_gas(
			&Value::null(),
			&Value::null(),
			&mut vec![data.get(data.get(i)?)?],
		)?;
	}
	unsafe { REACTION_INFO = Some(get_reaction_info()) };
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

pub fn with_specific_heats<T>(f: impl FnOnce(&FlatSimdVec) -> T) -> T {
	f(SPECIFIC_HEATS.read().as_ref().unwrap())
}

/// Returns the total number of gases in use. Only used by gas mixtures; should probably stay that way.
pub fn total_num_gases() -> GasIDX {
	TOTAL_NUM_GASES.load(Ordering::Acquire)
}

pub fn visibility_copies<'a>() -> Vec<Option<f32>> {
	GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.iter()
		.map(|g| g.moles_visible)
		.collect()
}

pub fn with_fire_products(f: impl FnOnce(&Vec<FlatSimdVec>)) {
	if let Some(products) = FIRE_PRODUCTS.read().as_ref() {
		f(products)
	} else {
		update_gas_refs();
		let mut v = Vec::with_capacity(total_num_gases());
		GAS_INFO_BY_IDX
			.read()
			.as_ref()
			.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
			.iter()
			.for_each(|gas| {
				let mut our_gases_vec = FlatSimdVec::new();
				if let Some(products) = gas.fire_products.as_ref() {
					for (id, amt) in products.iter() {
						our_gases_vec.force_set(id.get().unwrap(), *amt);
					}
				}
				v.push(our_gases_vec);
			});
		let res = f(&v);
		*FIRE_PRODUCTS.write() = Some(v);
		res
	}
}

pub fn with_gas_info<T>(f: impl FnOnce(&Vec<GasType>) -> T) -> T {
	f(GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!")))
}

pub fn update_gas_refs() {
	GAS_INFO_BY_IDX
		.write()
		.as_mut()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.iter_mut()
		.for_each(|gas| {
			if let Some(products) = gas.fire_products.as_mut() {
				for product in products.iter_mut() {
					product.0.update().unwrap();
				}
			}
		})
}

#[hook("/proc/finalize_gas_refs")]
fn _finalize_gas_refs() {
	update_gas_refs();
	Ok(Value::null())
}

thread_local! {
	static CACHED_GAS_IDS: RefCell<HashMap<u32, GasIDX, FxBuildHasher>> = RefCell::new(HashMap::with_hasher(FxBuildHasher::default()));
}

#[shutdown]
fn _delete_gas_ids() {
	CACHED_GAS_IDS.with(|c| {
		c.borrow_mut().clear();
	})
}

pub fn gas_idx_from_string(id: &str) -> Result<GasIDX, Runtime> {
	Ok(unsafe { GAS_INFO_BY_STRING.as_ref() }
		.ok_or_else(|| runtime!("Gases not loaded yet! Uh oh!"))?
		.get(id)
		.ok_or_else(|| runtime!("Invalid gas ID: {}", id))?
		.idx
		.clone())
}

/// Returns the appropriate index to be used by the game for a given ID string.
pub fn gas_idx_from_value(string_val: &Value) -> Result<GasIDX, Runtime> {
	CACHED_GAS_IDS.with(|c| {
		let mut cache = c.borrow_mut();
		let data = unsafe { string_val.raw.data.id }; // primarily to minimize chance of refcount funnies
		if let Some(idx) = cache.get(&data) {
			Ok(*idx)
		} else {
			let id = &string_val.as_string()?;
			let idx = gas_idx_from_string(id)?;
			cache.insert(data, idx);
			Ok(idx)
		}
	})
}

/// Takes an index and returns a borrowed string representing the string ID of the gas datum stored in that index.
pub fn gas_idx_to_id(idx: GasIDX) -> Result<&'static str, Runtime> {
	Ok(GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.ok_or_else(|| runtime!("Gases not loaded yet! Uh oh!"))?
		.get(idx as usize)
		.ok_or_else(|| runtime!("Invalid gas index: {}", idx))?
		.id)
}
