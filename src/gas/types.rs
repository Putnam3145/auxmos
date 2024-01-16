use auxcallback::byond_callback_sender;
use byondapi::prelude::*;

use fxhash::FxBuildHasher;

use parking_lot::{const_rwlock, RwLock};

use crate::reaction::{Reaction, ReactionPriority};

use super::GasIDX;

use dashmap::DashMap;

use std::{
	cell::RefCell,
	collections::BTreeMap,
	sync::atomic::{AtomicUsize, Ordering},
};

use hashbrown::HashMap;

use eyre::{Context, Result};

static TOTAL_NUM_GASES: AtomicUsize = AtomicUsize::new(0);

static REACTION_INFO: RwLock<Option<BTreeMap<ReactionPriority, Reaction>>> = const_rwlock(None);

/// The temperature at which this gas can oxidize and how much fuel it can oxidize when it can.
#[derive(Clone, Copy)]
pub struct OxidationInfo {
	temperature: f32,
	power: f32,
}

impl OxidationInfo {
	#[must_use]
	pub fn temperature(&self) -> f32 {
		self.temperature
	}
	#[must_use]
	pub fn power(&self) -> f32 {
		self.power
	}
}

/// The temperature at which this gas can burn and how much it burns when it does.
/// This may seem redundant with `OxidationInfo`, but `burn_rate` is actually the inverse, dimensions-wise, moles^-1 rather than moles.
#[derive(Clone, Copy)]
pub struct FuelInfo {
	temperature: f32,
	burn_rate: f32,
}

impl FuelInfo {
	#[must_use]
	pub fn temperature(&self) -> f32 {
		self.temperature
	}
	#[must_use]
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
	/// Gets the index of the gas.
	/// # Errors
	/// Propagates error from `gas_idx_from_string`.
	pub fn get(&self) -> Result<GasIDX> {
		match self {
			Self::Deferred(s) => gas_idx_from_string(s),
			Self::Found(id) => Ok(*id),
		}
	}
	/// Like `get`, but also caches the result if found.
	/// # Errors
	/// If the string is not a valid gas name.
	pub fn update(&mut self) -> Result<GasIDX> {
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
pub enum FireProductInfo {
	Generic(Vec<(GasRef, f32)>),
	Plasma, // yeah, just hardcoding the funny trit production
}

/// An individual gas type. Contains a whole lot of info attained from Byond when the gas is first registered.
/// If you don't have any of these, just fork auxmos and remove them, many of these are not necessary--for example,
/// if you don't have fusion, you can just remove `fusion_power`.
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
	/// Standard enthalpy of formation.
	/// Byond: `fire_energy_released`, a number.
	pub enthalpy: f32,
	/// Amount of radiation released per mole burned.
	/// Byond: `fire_radiation_released`, a number.
	pub fire_radiation_released: f32,
	/// Either fuel info, oxidation info or neither. See the documentation on the respective types.
	/// Byond: `oxidation_temperature` and `oxidation_rate` XOR `fire_temperature` and `fire_burn_rate`
	pub fire_info: FireInfo,
	/// A vector of gas-amount pairs. GasRef is just which gas, the f32 is moles made/mole burned.
	/// Byond: `fire_products`, a list of gas IDs associated with amounts.
	pub fire_products: Option<FireProductInfo>,
}

impl GasType {
	// This absolute monster is what you want to override to add or remove certain gas properties, based on what a gas datum has.
	fn new(gas: &ByondValue, idx: GasIDX) -> Result<Self> {
		Ok(Self {
			idx,
			id: gas.read_string_id(byond_string!("id"))?.into_boxed_str(),
			name: gas.read_string_id(byond_string!("name"))?.into_boxed_str(),
			flags: gas
				.read_number_id(byond_string!("flags"))
				.unwrap_or_default() as u32,
			specific_heat: gas.read_number_id(byond_string!("specific_heat"))?,
			fusion_power: gas
				.read_number_id(byond_string!("fusion_power"))
				.unwrap_or_default(),
			moles_visible: gas.read_number_id(byond_string!("moles_visible")).ok(),
			fire_info: {
				if let Ok(temperature) = gas.read_number_id(byond_string!("oxidation_temperature"))
				{
					FireInfo::Oxidation(OxidationInfo {
						temperature,
						power: gas.read_number_id(byond_string!("oxidation_rate"))?,
					})
				} else if let Ok(temperature) =
					gas.read_number_id(byond_string!("fire_temperature"))
				{
					FireInfo::Fuel(FuelInfo {
						temperature,
						burn_rate: gas.read_number_id(byond_string!("fire_burn_rate"))?,
					})
				} else {
					FireInfo::None
				}
			},
			fire_products: gas
				.read_var_id(byond_string!("fire_products"))
				.ok()
				.and_then(|product_info| {
					if product_info.is_list() {
						Some(FireProductInfo::Generic(
							product_info
								.iter()
								.unwrap()
								.filter_map(|(k, v)| {
									k.get_string().ok().and_then(|s_str| {
										v.get_number()
											.ok()
											.map(|amt| (GasRef::Deferred(s_str), amt))
									})
								})
								.collect(),
						))
					} else if product_info.is_num() {
						Some(FireProductInfo::Plasma) // if we add another snowflake later, add it, but for now we hack this in
					} else {
						None
					}
				}),
			enthalpy: gas
				.read_number_id(byond_string!("enthalpy"))
				.unwrap_or_default(),
			fire_radiation_released: gas
				.read_number_id(byond_string!("fire_radiation_released"))
				.unwrap_or_default(),
		})
	}
}

static GAS_INFO_BY_STRING: RwLock<Option<DashMap<Box<str>, GasType, FxBuildHasher>>> =
	const_rwlock(None);

static GAS_INFO_BY_IDX: RwLock<Option<Vec<GasType>>> = const_rwlock(None);

static GAS_SPECIFIC_HEATS: RwLock<Option<Vec<f32>>> = const_rwlock(None);

pub fn initialize_gas_info_structs() {
	*GAS_INFO_BY_STRING.write() = Some(DashMap::with_hasher(FxBuildHasher::default()));
	*GAS_INFO_BY_IDX.write() = Some(Vec::new());
	*GAS_SPECIFIC_HEATS.write() = Some(Vec::new());
}

pub fn destroy_gas_info_structs() {
	crate::turfs::wait_for_tasks();
	GAS_INFO_BY_STRING.write().as_mut().unwrap().clear();
	GAS_INFO_BY_IDX.write().as_mut().unwrap().clear();
	GAS_SPECIFIC_HEATS.write().as_mut().unwrap().clear();
	TOTAL_NUM_GASES.store(0, Ordering::Release);
	CACHED_GAS_IDS.with_borrow_mut(|gas_ids| {
		gas_ids.clear();
	});
	CACHED_IDX_TO_STRINGS.with_borrow_mut(|gas_ids| {
		gas_ids.clear();
	});
}

#[byondapi_binds::bind("/proc/_auxtools_register_gas")]
fn hook_register_gas(gas: ByondValue) {
	let gas_id = gas.read_string_id(byond_string!("id"))?;
	match {
		GAS_INFO_BY_STRING
			.read()
			.as_ref()
			.unwrap()
			.get_mut(&gas_id as &str)
	} {
		Some(mut old_gas) => {
			let gas_cache = GasType::new(&gas, old_gas.idx)?;
			*old_gas = gas_cache.clone();
			GAS_SPECIFIC_HEATS.write().as_mut().unwrap()[old_gas.idx] = gas_cache.specific_heat;
			GAS_INFO_BY_IDX.write().as_mut().unwrap()[old_gas.idx] = gas_cache;
		}
		None => {
			let gas_cache = GasType::new(&gas, TOTAL_NUM_GASES.load(Ordering::Acquire))?;
			let cached_id = gas_id.clone();
			let cached_idx = gas_cache.idx;
			GAS_INFO_BY_STRING
				.read()
				.as_ref()
				.unwrap()
				.insert(gas_id.into_boxed_str(), gas_cache.clone());
			GAS_SPECIFIC_HEATS
				.write()
				.as_mut()
				.unwrap()
				.push(gas_cache.specific_heat);
			GAS_INFO_BY_IDX.write().as_mut().unwrap().push(gas_cache);
			CACHED_IDX_TO_STRINGS
				.with_borrow_mut(|map| map.insert(cached_idx, cached_id.into_boxed_str()));
			TOTAL_NUM_GASES.fetch_add(1, Ordering::Release); // this is the only thing that stores it other than shutdown
		}
	}
	Ok(ByondValue::null())
}

#[byondapi_binds::bind("/proc/auxtools_atmos_init")]
fn hook_init(gas_data: ByondValue) {
	let data = gas_data.read_var_id(byond_string!("datums"))?;
	data.iter()?
		.map(|(_, gas)| hook_register_gas(gas))
		.try_for_each(|res| res.map(drop))
		.wrap_err("auxtools_atmos_init failed to register gas")?;
	*REACTION_INFO.write() = Some(get_reaction_info());
	Ok(true.into())
}

fn get_reaction_info() -> BTreeMap<ReactionPriority, Reaction> {
	let gas_reactions = ByondValue::new_global_ref()
		.read_var_id(byond_string!("SSair"))
		.unwrap()
		.read_var_id(byond_string!("gas_reactions"))
		.unwrap();
	let mut reaction_cache: BTreeMap<ReactionPriority, Reaction> = Default::default();
	let sender = byond_callback_sender();
	for (reaction, _) in gas_reactions.iter().unwrap() {
		match Reaction::from_byond_reaction(reaction) {
			Ok(reaction) => {
				if let std::collections::btree_map::Entry::Vacant(e) =
					reaction_cache.entry(reaction.get_priority())
				{
					e.insert(reaction);
				} else {
					drop(sender.try_send(Box::new(move || {
						Err(eyre::eyre!(format!(
							"Duplicate reaction priority {}, this reaction will be ignored!",
							reaction.get_priority().0
						)))
					})));
				}
			}
			//maybe awful error handling
			Err(runtime) => {
				drop(sender.try_send(Box::new(move || Err(runtime))));
			}
		}
	}
	reaction_cache
}

#[byondapi_binds::bind("/datum/controller/subsystem/air/proc/auxtools_update_reactions")]
fn update_reactions() {
	*REACTION_INFO.write() = Some(get_reaction_info());
	Ok(true.into())
}

/// Calls the given closure with all reaction info as an argument.
/// # Panics
/// If reactions aren't loaded yet.
pub fn with_reactions<T, F>(mut f: F) -> T
where
	F: FnMut(&BTreeMap<ReactionPriority, Reaction>) -> T,
{
	f(REACTION_INFO
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Reactions not loaded yet! Uh oh!")))
}

/// Runs the given closure with the global specific heats vector locked.
/// # Panics
/// If gas info isn't loaded yet.
pub fn with_specific_heats<T>(f: impl FnOnce(&[f32]) -> T) -> T {
	f(GAS_SPECIFIC_HEATS.read().as_ref().unwrap().as_slice())
}

/// Gets the fusion power of the given gas.
/// # Panics
/// If gas info isn't loaded yet.
#[cfg(feature = "reaction_hooks")]
#[must_use]
pub fn gas_fusion_power(idx: &GasIDX) -> f32 {
	GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.get(*idx)
		.unwrap()
		.fusion_power
}

/// Returns the total number of gases in use. Only used by gas mixtures; should probably stay that way.
pub fn total_num_gases() -> GasIDX {
	TOTAL_NUM_GASES.load(Ordering::Acquire)
}

/// Gets the gas visibility threshold for the given gas ID.
/// # Panics
/// If gas info isn't loaded yet.
#[must_use]
pub fn gas_visibility(idx: usize) -> Option<f32> {
	GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.get(idx)
		.unwrap()
		.moles_visible
}

/// Gets a copy of all the gas visibilities.
/// # Panics
/// If gas info isn't loaded yet.
#[must_use]
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
/// # Panics
/// If gas info isn't loaded yet.
pub fn with_gas_info<T>(f: impl FnOnce(&[GasType]) -> T) -> T {
	f(GAS_INFO_BY_IDX
		.read()
		.as_ref()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!")))
}

/// Updates all the `GasRef`s in the global gas info vec with proper indices instead of strings.
/// # Panics
/// If gas info is not loaded yet.
pub fn update_gas_refs() {
	GAS_INFO_BY_IDX
		.write()
		.as_mut()
		.unwrap_or_else(|| panic!("Gases not loaded yet! Uh oh!"))
		.iter_mut()
		.for_each(|gas| {
			if let Some(FireProductInfo::Generic(products)) = gas.fire_products.as_mut() {
				for product in products.iter_mut() {
					product.0.update().unwrap();
				}
			}
		});
}

#[byondapi_binds::bind("/proc/finalize_gas_refs")]
fn finalize_gas_refs() {
	update_gas_refs();
	Ok(ByondValue::null())
}

thread_local! {
	static CACHED_GAS_IDS: RefCell<HashMap<u32, GasIDX, FxBuildHasher>> = RefCell::new(HashMap::with_hasher(FxBuildHasher::default()));
	static CACHED_IDX_TO_STRINGS: RefCell<HashMap<usize,Box<str>, FxBuildHasher>> = RefCell::new(HashMap::with_hasher(FxBuildHasher::default()));
}

/// Returns the appropriate index to be used by auxmos for a given ID string.
/// # Errors
/// If gases aren't loaded or an invalid gas ID is given.
pub fn gas_idx_from_string(id: &str) -> Result<GasIDX> {
	Ok(GAS_INFO_BY_STRING
		.read()
		.as_ref()
		.ok_or_else(|| eyre::eyre!("Gases not loaded yet! Uh oh!"))?
		.get(id)
		.ok_or_else(|| eyre::eyre!("Invalid gas ID: {id}"))?
		.idx)
}

/// Returns the appropriate index to be used by the game for a given Byond string.
/// # Errors
/// If the given string is not a string or is not a valid gas ID.
pub fn gas_idx_from_value(string_val: &ByondValue) -> Result<GasIDX> {
	CACHED_GAS_IDS.with_borrow_mut(|cache| {
		if let Some(idx) = cache.get(&string_val.get_strid().unwrap()) {
			Ok(*idx)
		} else {
			let id = &string_val.get_string()?;
			let idx = gas_idx_from_string(id)?;
			cache.insert(string_val.get_strid().unwrap(), idx);
			Ok(idx)
		}
	})
}

/// Takes an index and returns a borrowed string representing the string ID of the gas datum stored in that index.
/// # Panics
/// If an invalid gas index is given to this. This should never happen, so we panic instead of runtiming.
pub fn gas_idx_to_id(idx: GasIDX) -> ByondValue {
	CACHED_IDX_TO_STRINGS.with_borrow(|stuff| {
		ByondValue::new_str(
			stuff
				.get(&idx)
				.unwrap_or_else(|| panic!("Invalid gas index: {idx}"))
				.as_ref(),
		)
		.unwrap_or_else(|_| panic!("Cannot convert gas index to byond string: {idx}"))
	})
}

#[cfg(test)]
pub fn register_gas_manually(gas_id: &'static str, specific_heat: f32) {
	let gas_cache = GasType {
		idx: total_num_gases(),
		id: gas_id.into(),
		name: gas_id.into(),
		flags: 0,
		specific_heat,
		fusion_power: 0.0,
		moles_visible: None,
		enthalpy: 0.0,
		fire_radiation_released: 0.0,
		fire_info: FireInfo::None,
		fire_products: None,
	};
	let cached_idx = gas_cache.idx;
	GAS_INFO_BY_STRING
		.read()
		.as_ref()
		.unwrap()
		.insert(gas_id.into(), gas_cache.clone());

	GAS_SPECIFIC_HEATS
		.write()
		.as_mut()
		.unwrap()
		.push(gas_cache.specific_heat);
	GAS_INFO_BY_IDX.write().as_mut().unwrap().push(gas_cache);
	CACHED_IDX_TO_STRINGS.with_borrow_mut(|map| map.insert(cached_idx, gas_id.into()));
	TOTAL_NUM_GASES.fetch_add(1, Ordering::Release); // this is the only thing that stores it other than shutdown
}

#[cfg(test)]
pub fn set_gas_statics_manually() {
	initialize_gas_info_structs();
}

#[cfg(test)]
pub fn destroy_gas_statics() {
	destroy_gas_info_structs();
}
