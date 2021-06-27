use std::sync::atomic::{AtomicUsize, Ordering};

use fxhash::FxBuildHasher;

use std::cell::RefCell;

use super::reaction::Reaction;

use super::GasIDX;

use dashmap::DashMap;

use std::collections::HashMap;

static TOTAL_NUM_GASES: AtomicUsize = AtomicUsize::new(0);

static REACTION_INFO: RwLock<Option<Vec<Reaction>>> = const_rwlock(None);

static REAGENT_GASES: RwLock<Option<Vec<GasIDX>>> = const_rwlock(None);

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
	pub id: Box<str>,
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

impl GasType {
	fn new(gas: &Value, idx: GasIDX) -> Result<Self, Runtime> {
		if gas
			.get(byond_string!("turf_reagent"))
			.map_or(false, |v| v != Value::null())
		{
			if let Some(reagent_gases) = REAGENT_GASES.write().as_mut() {
				reagent_gases.push(idx);
				reagent_gases.sort_unstable();
				reagent_gases.dedup();
			}
		}
		Ok(Self {
			idx,
			id: gas.get_string(byond_string!("id"))?.into_boxed_str(),
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
					(1..=products.len())
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

static GAS_SPECIFIC_HEATS: RwLock<Option<Vec<f32>>> = const_rwlock(None);

#[init(partial)]
fn _create_gas_info_structs() -> Result<(), String> {
	unsafe {
		GAS_INFO_BY_STRING = Some(DashMap::with_hasher(FxBuildHasher::default()));
	};
	*GAS_INFO_BY_IDX.write() = Some(Vec::new());
	*GAS_SPECIFIC_HEATS.write() = Some(Vec::new());
	*REAGENT_GASES.write() = Some(Vec::new());
	Ok(())
}

#[shutdown]
fn _destroy_gas_info_structs() {
	unsafe {
		GAS_INFO_BY_STRING = None;
	};
	*GAS_INFO_BY_IDX.write() = None;
	*GAS_SPECIFIC_HEATS.write() = None;
	*REAGENT_GASES.write() = None;
	TOTAL_NUM_GASES.store(0, Ordering::Release);
	CACHED_GAS_IDS.with(|gas_ids| {
		gas_ids.borrow_mut().clear();
	});
}

#[hook("/proc/_auxtools_register_gas")]
fn _hook_register_gas(gas: Value) {
	let gas_id = gas.get_string(byond_string!("id")).unwrap();
	let gas_cache = GasType::new(gas, TOTAL_NUM_GASES.load(Ordering::Relaxed))?;
	unsafe { GAS_INFO_BY_STRING.as_ref() }
		.unwrap()
		.insert(gas_id.into_boxed_str(), gas_cache.clone());
	GAS_SPECIFIC_HEATS
		.write()
		.as_mut()
		.unwrap()
		.push(gas_cache.specific_heat);
	GAS_INFO_BY_IDX.write().as_mut().unwrap().push(gas_cache);
	TOTAL_NUM_GASES.fetch_add(1, Ordering::Release); // this is the only thing that stores it other than shutdown
	Ok(Value::null())
}

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
	*REACTION_INFO.write() = Some(get_reaction_info());
	Ok(Value::from(true))
}

fn get_reaction_info() -> Vec<Reaction> {
	let gas_reactions = Value::globals()
		.get(byond_string!("SSair"))
		.unwrap()
		.get_list(byond_string!("gas_reactions"))
		.unwrap();
	let mut reaction_cache: Vec<Reaction> = Vec::with_capacity(gas_reactions.len() as usize);
	for i in 1..=gas_reactions.len() {
		let reaction = &gas_reactions.get(i).unwrap();
		reaction_cache.push(Reaction::from_byond_reaction(reaction));
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
	F: FnMut(&[Reaction]) -> T,
{
	f(REACTION_INFO
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Reactions not loaded yet! Uh oh!")))
}

pub fn with_specific_heats<T>(f: impl FnOnce(&[f32]) -> T) -> T {
	f(GAS_SPECIFIC_HEATS.read().as_ref().unwrap().as_slice())
}

pub fn with_reagent_gases<T>(f: impl FnOnce(&[usize]) -> T) -> T {
	f(REAGENT_GASES.read().as_ref().unwrap().as_slice())
}

#[cfg(feature = "reaction_hooks")]
pub fn gas_fusion_power(idx: &GasIDX) -> f32 {
	GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.get(*idx as usize)
		.unwrap()
		.fusion_power
}

/// Returns the total number of gases in use. Only used by gas mixtures; should probably stay that way.
pub fn total_num_gases() -> GasIDX {
	TOTAL_NUM_GASES.load(Ordering::Acquire)
}

/// Gets the gas visibility threshold for the given gas ID.
pub fn gas_visibility(idx: usize) -> Option<f32> {
	GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.get(idx as usize)
		.unwrap()
		.moles_visible
}

pub fn visibility_copies() -> Vec<Option<f32>> {
	GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.iter()
		.map(|g| g.moles_visible)
		.collect()
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
		});
}

#[hook("/proc/finalize_gas_refs")]
fn _finalize_gas_refs() {
	update_gas_refs();
	Ok(Value::null())
}

thread_local! {
	static CACHED_GAS_IDS: RefCell<HashMap<Value, GasIDX, FxBuildHasher>> = RefCell::new(HashMap::with_hasher(FxBuildHasher::default()));
}

pub fn gas_idx_from_string(id: &str) -> Result<GasIDX, Runtime> {
	Ok(unsafe { GAS_INFO_BY_STRING.as_ref() }
		.ok_or_else(|| runtime!("Gases not loaded yet! Uh oh!"))?
		.get(id)
		.ok_or_else(|| runtime!("Invalid gas ID: {}", id))?
		.idx)
}

/// Returns the appropriate index to be used by the game for a given ID string.
pub fn gas_idx_from_value(string_val: &Value) -> Result<GasIDX, Runtime> {
	CACHED_GAS_IDS.with(|c| {
		let mut cache = c.borrow_mut();
		if let Some(idx) = cache.get(string_val) {
			Ok(*idx)
		} else {
			let id = &string_val.as_string()?;
			let idx = gas_idx_from_string(id)?;
			cache.insert(string_val.clone(), idx);
			Ok(idx)
		}
	})
}

/// Takes an index and returns a borrowed string representing the string ID of the gas datum stored in that index.
pub fn gas_idx_to_id(
	idx: GasIDX,
) -> Result<parking_lot::MappedRwLockReadGuard<'static, Box<str>>, Runtime> {
	Ok(parking_lot::RwLockReadGuard::map(
		GAS_INFO_BY_IDX.read(),
		|lock| {
			&lock
				.as_ref()
				.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
				.get(idx as usize)
				.unwrap_or_else(|| panic!("Invalid gas index: {}", idx))
				.id
		},
	))
}
