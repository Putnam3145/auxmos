use std::sync::atomic::{AtomicUsize, Ordering};

use fxhash::FxBuildHasher;

use std::cell::RefCell;

use crate::reaction::Reaction;

use super::GasIDX;

use dashmap::DashMap;

use std::collections::HashMap;

static TOTAL_NUM_GASES: AtomicUsize = AtomicUsize::new(0);

static mut REACTION_INFO: Option<Vec<Reaction>> = None;

use auxtools::*;

use parking_lot::{const_rwlock, RwLock};

/// The temperature at which this gas can oxidize and how much fuel it can oxidize when it can.
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

/// The temperature at which this gas can burn and how much it burns when it does.
/// This may seem redundant with OxidationInfo, but burn_rate is actually the inverse, dimensions-wise, moles^-1 rather than moles.
#[derive(Clone, Copy)]
pub struct FuelInfo {
	temperature: f32,
	burn_rate: f32,
}

impl FuelInfo {
	pub fn temperature(&self) -> f32 {
		self.temperature
	}
	pub fn burn_rate(&self) -> f32 {
		self.burn_rate
	}
}

/// Contains either oxidation info, fuel info or neither.
/// Just use it with match always, no helpers here.
#[derive(Clone, Copy)]
pub enum FireInfo {
	Oxidation(OxidationInfo),
	Fuel(FuelInfo),
	None,
}

/// We can't guarantee the order of loading gases, so any properties of gases that reference other gases
/// must have a reference like this that can get the proper index at runtime.
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

/// An individual gas type. Contains a whole lot of info attained from Byond when the gas is first registered.
/// If you don't have any of these, just fork auxmos and remove them, many of these are not necessary--for example,
/// if you don't have fusion, you can just remove fusion_power.
/// Each individual member also has the byond /datum/gas equivalent listed.
#[derive(Clone)]
pub struct GasType {
	/// The index of this gas in the moles vector of a mixture. Usually the most common representation in Auxmos, for speed.
	/// No byond equivalent.
	pub idx: GasIDX,
	/// The ID on the byond end, as a boxed str. Most convenient way to reference it in code; use the function gas_idx_from_string to get idx from this.
	/// Byond: `id`, a string.
	pub id: Box<str>,
	/// The gas's name. Not used in auxmos as of yet.
	/// Byond: `name`, a string.
	pub name: Box<str>,
	/// Not used in auxmos, there for completeness. Only flag on Citadel is GAS_DANGEROUS.
	/// Byond: `flags`, a number (bitflags).
	pub flags: u32,
	/// The specific heat of the gas. Duplicated in the GAS_SPECIFIC_HEATS vector for speed.
	/// Byond: `specific_heat`, a number.
	pub specific_heat: f32,
	/// Gas's fusion power. Used in fusion hooking, so this can be removed and ignored if you don't have fusion.
	/// Byond: `fusion_power`, a number.
	pub fusion_power: f32,
	/// The moles at which the gas's overlay or other appearance shows up. If None, gas is never visible.
	/// Byond: `moles_visible`, a number.
	pub moles_visible: Option<f32>,
	/// Amount of energy released per mole of material burned in generic fires.
	/// Byond: `fire_energy_released`, a number.
	pub fire_energy_released: f32,
	/// Either fuel info, oxidation info or neither. See the documentation on the respective types.
	/// Byond: `oxidation_temperature` and `oxidation_rate` XOR `fire_temperature` and `fire_burn_rate`
	pub fire_info: FireInfo,
	/// A vector of gas-amount pairs. GasRef is just which gas, the f32 is moles made/mole burned.
	/// Byond: `fire_products`, a list of gas IDs associated with amounts.
	pub fire_products: Option<Vec<(GasRef, f32)>>,
}

impl GasType {
	// This absolute monster is what you want to override to add or remove certain gas properties, based on what a gas datum has.
	fn new(gas: &Value, idx: GasIDX) -> Result<Self, Runtime> {
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
			fire_info: {
				if let Ok(temperature) = gas.get_number(byond_string!("oxidation_temperature")) {
					FireInfo::Oxidation(OxidationInfo {
						temperature,
						power: gas.get_number(byond_string!("oxidation_rate"))?,
					})
				} else if let Ok(temperature) = gas.get_number(byond_string!("fire_temperature")) {
					FireInfo::Fuel(FuelInfo {
						temperature,
						burn_rate: gas.get_number(byond_string!("fire_burn_rate"))?,
					})
				} else {
					FireInfo::None
				}
			},
			fire_products: if let Ok(products) = gas.get_list(byond_string!("fire_products")) {
				Some(
					(1..=products.len())
						.filter_map(|i| {
							let s = products.get(i).unwrap();
							s.as_string()
								.and_then(|s_str| {
									products
										.get(s)
										.and_then(|v| v.as_number())
										.map(|amount| (GasRef::Deferred(s_str), amount))
								})
								.ok()
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
	Ok(())
}

#[shutdown]
fn _destroy_gas_info_structs() {
	unsafe {
		GAS_INFO_BY_STRING = None;
	};
	*GAS_INFO_BY_IDX.write() = None;
	*GAS_SPECIFIC_HEATS.write() = None;
	TOTAL_NUM_GASES.store(0, Ordering::Release);
	CACHED_GAS_IDS.with(|gas_ids| {
		gas_ids.borrow_mut().clear();
	});
}

#[hook("/proc/_auxtools_register_gas")]
fn _hook_register_gas(gas: Value) {
	let gas_id = gas.get_string(byond_string!("id"))?;
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
	unsafe {
		REACTION_INFO = Some(get_reaction_info());
	};
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
	unsafe {
		REACTION_INFO = Some(get_reaction_info());
	};
	Ok(Value::from(true))
}

pub fn with_reactions<T, F>(mut f: F) -> T
where
	F: FnMut(&[Reaction]) -> T,
{
	f(unsafe { REACTION_INFO.as_ref() }
		.unwrap_or_else(|| panic!("Reactions not loaded yet! Uh oh!")))
}

pub fn with_specific_heats<T>(f: impl FnOnce(&[f32]) -> T) -> T {
	f(GAS_SPECIFIC_HEATS.read().as_ref().unwrap().as_slice())
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

/// Gets a copy of all the gas visibilities.
pub fn visibility_copies() -> Box<[Option<f32>]> {
	GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.iter()
		.map(|g| g.moles_visible)
		.collect::<Vec<_>>()
		.into_boxed_slice()
}

/// Allows one to run a closure with a lock on the global gas info vec.
pub fn with_gas_info<T>(f: impl FnOnce(&[GasType]) -> T) -> T {
	f(GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!")))
}

/// Updates all the GasRefs in the global gas info vec with proper indices instead of strings.
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

/// Returns the appropriate index to be used by auxmos for a given ID string.
pub fn gas_idx_from_string(id: &str) -> Result<GasIDX, Runtime> {
	Ok(unsafe { GAS_INFO_BY_STRING.as_ref() }
		.ok_or_else(|| runtime!("Gases not loaded yet! Uh oh!"))?
		.get(id)
		.ok_or_else(|| runtime!("Invalid gas ID: {}", id))?
		.idx)
}

/// Returns the appropriate index to be used by the game for a given Byond string.
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
