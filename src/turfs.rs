pub mod groups;
pub mod processing;
/*
#[cfg(feature = "monstermos")]
mod monstermos;
#[cfg(feature = "putnamos")]
mod putnamos;
*/
#[cfg(feature = "katmos")]
pub mod katmos;
#[cfg(feature = "superconductivity")]
mod superconduct;

use crate::{constants::*, gas::Mixture, GasArena};
use bitflags::bitflags;
use byondapi::prelude::*;
use eyre::{Context, Result};
use indexmap::IndexMap;
use parking_lot::{const_rwlock, RwLock, RwLockUpgradableReadGuard};
use petgraph::{graph::NodeIndex, stable_graph::StableDiGraph, visit::EdgeRef, Direction};
use rayon::prelude::*;
use rustc_hash::FxBuildHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use std::{mem::drop, sync::atomic::AtomicU64};

bitflags! {
	#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
	pub struct Directions: u8 {
		const NORTH = 0b1;
		const SOUTH = 0b10;
		const EAST	= 0b100;
		const WEST	= 0b1000;
		const UP 	= 0b10000;
		const DOWN 	= 0b100000;
		const ALL_CARDINALS = Self::NORTH.bits() | Self::SOUTH.bits() | Self::EAST.bits() | Self::WEST.bits();
		const ALL_CARDINALS_MULTIZ = Self::NORTH.bits() | Self::SOUTH.bits() | Self::EAST.bits() | Self::WEST.bits() | Self::UP.bits() | Self::DOWN.bits();
	}

	#[derive(Default, Debug)]
	pub struct SimulationFlags: u8 {
		const SIMULATION_DIFFUSE = 0b1;
		const SIMULATION_ALL = 0b10;
		const SIMULATION_ANY = Self::SIMULATION_DIFFUSE.bits() | Self::SIMULATION_ALL.bits();
	}

	#[derive(Default, Debug)]
	pub struct AdjacentFlags: u8 {
		const ATMOS_ADJACENT_FIRELOCK = 0b10;
	}

	#[derive(Default, Debug, Clone, Copy)]
	pub struct DirtyFlags: u8 {
		const DIRTY_MIX_REF = 0b1;
		const DIRTY_ADJACENT = 0b10;
		const DIRTY_ADJACENT_TO_SPACE = 0b100;
	}
}

#[allow(unused)]
const fn adj_flag_to_idx(adj_flag: Directions) -> u8 {
	match adj_flag {
		Directions::NORTH => 0,
		Directions::SOUTH => 1,
		Directions::EAST => 2,
		Directions::WEST => 3,
		Directions::UP => 4,
		Directions::DOWN => 5,
		_ => 6,
	}
}

#[allow(unused)]
const fn idx_to_adj_flag(idx: u8) -> Directions {
	match idx {
		0 => Directions::NORTH,
		1 => Directions::SOUTH,
		2 => Directions::EAST,
		3 => Directions::WEST,
		4 => Directions::UP,
		5 => Directions::DOWN,
		_ => Directions::from_bits_truncate(0),
	}
}

type TurfID = u32;

// TurfMixture can be treated as "immutable" for all intents and purposes--put other data somewhere else
#[derive(Default, Debug)]
struct TurfMixture {
	pub mix: usize,
	pub id: TurfID,
	pub flags: SimulationFlags,
	pub planetary_atmos: Option<u32>,
	pub vis_hash: AtomicU64,
}

#[allow(dead_code)]
impl TurfMixture {
	/// Whether the turf is processed at all or not
	pub fn enabled(&self) -> bool {
		self.flags.intersects(SimulationFlags::SIMULATION_ANY)
	}

	/// Whether the turf's gas is immutable or not, see [`super::gas::Mixture`]
	pub fn is_immutable(&self) -> bool {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.is_immutable()
		})
	}
	/// Returns the pressure of the turf's gas, see [`super::gas::Mixture`]
	pub fn return_pressure(&self) -> f32 {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.return_pressure()
		})
	}
	/// Returns the temperature of the turf's gas, see [`super::gas::Mixture`]
	pub fn return_temperature(&self) -> f32 {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.get_temperature()
		})
	}
	/// Returns the total moles of the turf's gas, see [`super::gas::Mixture`]
	pub fn total_moles(&self) -> f32 {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.total_moles()
		})
	}
	/// Clears the turf's airs, see [`super::gas::Mixture`]
	pub fn clear_air(&self) {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.write()
				.clear();
		});
	}
	/// Copies from a given gas mixture to the turf's airs, see [`super::gas::Mixture`]
	pub fn copy_from_mutable(&self, sample: &Mixture) {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.write()
				.copy_from_mutable(sample);
		});
	}
	/// Clears a number of moles from the turf's air
	/// If the number of moles is greater than the turf's total moles, just clears the turf
	pub fn clear_moles(&self, amt: f32) {
		GasArena::with_all_mixtures(|all_mixtures| {
			let moles = all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.total_moles();
			if amt >= moles {
				all_mixtures
					.get(self.mix)
					.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
					.write()
					.clear();
			} else {
				drop(
					all_mixtures
						.get(self.mix)
						.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
						.write()
						.remove(amt),
				);
			}
		});
	}
	/// Gets a copy of the turf's airs, see [`super::gas::Mixture`]
	pub fn get_gas_copy(&self) -> Mixture {
		let mut ret: Mixture = Mixture::new();
		GasArena::with_all_mixtures(|all_mixtures| {
			let to_copy = all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read();
			ret.copy_from_mutable(&to_copy);
			ret.volume = to_copy.volume;
		});
		ret
	}
	/// Invalidates the turf's visibility cache
	/// This turf will most likely be visually updated the next processing cycle
	/// If that is even running
	pub fn invalidate_vis_cache(&self) {
		self.vis_hash.store(0, std::sync::atomic::Ordering::Relaxed);
	}
}

type TurfGraphMap = IndexMap<TurfID, NodeIndex, FxBuildHasher>;

//adjacency/turf infos goes here
#[derive(Debug)]
struct TurfGases {
	graph: StableDiGraph<TurfMixture, AdjacentFlags>,
	map: TurfGraphMap,
}

impl TurfGases {
	pub fn insert_turf(&mut self, tmix: TurfMixture) {
		if let Some(&node_id) = self.map.get(&tmix.id) {
			let thin = self.graph.node_weight_mut(node_id).unwrap();
			*thin = tmix
		} else {
			self.map.insert(tmix.id, self.graph.add_node(tmix));
		}
	}
	pub fn remove_turf(&mut self, id: TurfID) {
		if let Some(index) = self.map.shift_remove(&id) {
			self.graph.remove_node(index);
		}
	}
	pub fn update_adjacencies(&mut self, idx: TurfID, adjacent_list: ByondValue) -> Result<()> {
		if let Some(&this_index) = self.map.get(&idx) {
			self.remove_adjacencies(this_index);
			adjacent_list
				.iter()?
				.filter_map(|(k, v)| Some((k.get_ref().ok()?, v.get_number().unwrap_or(0.0) as u8)))
				.filter_map(|(adj_ref, flag)| Some((self.map.get(&adj_ref)?, flag)))
				.for_each(|(adj_index, flag)| {
					let flags = AdjacentFlags::from_bits_truncate(flag);
					self.graph.add_edge(this_index, *adj_index, flags);
				})
		};
		Ok(())
	}

	pub fn remove_adjacencies(&mut self, index: NodeIndex) {
		let edges = self
			.graph
			.edges(index)
			.map(|edgeref| edgeref.id())
			.collect::<Vec<_>>();
		edges.into_iter().for_each(|edgeindex| {
			self.graph.remove_edge(edgeindex);
		});
	}

	pub fn get(&self, idx: NodeIndex) -> Option<&TurfMixture> {
		self.graph.node_weight(idx)
	}

	#[allow(unused)]
	pub fn get_from_id(&self, idx: TurfID) -> Option<&TurfMixture> {
		self.map
			.get(&idx)
			.and_then(|&idx| self.graph.node_weight(idx))
	}

	#[allow(unused)]
	pub fn get_id(&self, idx: TurfID) -> Option<NodeIndex> {
		self.map.get(&idx).copied()
	}

	pub fn adjacent_node_ids(&self, index: NodeIndex) -> impl Iterator<Item = NodeIndex> + '_ {
		self.graph.neighbors(index)
	}

	#[allow(unused)]
	pub fn adjacent_turf_ids(&self, index: NodeIndex) -> impl Iterator<Item = TurfID> + '_ {
		self.graph
			.neighbors(index)
			.filter_map(|index| Some(self.get(index)?.id))
	}

	#[allow(unused)]
	pub fn adjacent_node_ids_enabled(
		&self,
		index: NodeIndex,
	) -> impl Iterator<Item = NodeIndex> + '_ {
		self.graph.neighbors(index).filter(|&adj_index| {
			self.graph
				.node_weight(adj_index)
				.map_or(false, |mix| mix.enabled())
		})
	}

	pub fn adjacent_mixes<'a>(
		&'a self,
		index: NodeIndex,
		all_mixtures: &'a [parking_lot::RwLock<Mixture>],
	) -> impl Iterator<Item = &'a parking_lot::RwLock<Mixture>> {
		self.graph
			.neighbors(index)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
			.filter_map(move |idx| all_mixtures.get(idx.mix))
	}

	pub fn adjacent_mixes_with_adj_ids<'a>(
		&'a self,
		index: NodeIndex,
		all_mixtures: &'a [parking_lot::RwLock<Mixture>],
		dir: Direction,
	) -> impl Iterator<Item = (&'a TurfID, &'a parking_lot::RwLock<Mixture>)> {
		self.graph
			.neighbors_directed(index, dir)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
			.filter_map(move |idx| Some((&idx.id, all_mixtures.get(idx.mix)?)))
	}
	pub fn clear(&mut self) {
		self.graph.clear();
		self.map.clear();
	}

	/*
	pub fn adjacent_infos(
		&self,
		index: NodeIndex,
		dir: Direction,
	) -> impl Iterator<Item = &TurfMixture> {
		self.graph
			.neighbors_directed(index, dir)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
	}

	pub fn adjacent_ids<'a>(&'a self, idx: TurfID) -> impl Iterator<Item = &'a TurfID> {
		self.graph
			.neighbors(*self.map.get(&idx).unwrap())
			.filter_map(|index| self.graph.node_weight(index))
			.map(|tmix| &tmix.id)
	}
	pub fn adjacents_enabled<'a>(&'a self, idx: TurfID) -> impl Iterator<Item = &'a TurfID> {
		self.graph
			.neighbors(*self.map.get(&idx).unwrap())
			.filter_map(|index| self.graph.node_weight(index))
			.filter(|tmix| tmix.enabled())
			.map(|tmix| &tmix.id)
	}
	pub fn get_mixture(&self, idx: TurfID) -> Option<TurfMixture> {
		self.mixtures.read().get(&idx).cloned()
	}
	*/
}

static TURF_GASES: RwLock<Option<TurfGases>> = const_rwlock(None);

// We store planetary atmos by hash of the initial atmos string here for speed.
static PLANETARY_ATMOS: RwLock<Option<IndexMap<u32, Mixture, FxBuildHasher>>> = const_rwlock(None);

//whether there is any tasks running
static TASKS: RwLock<()> = const_rwlock(());

pub fn wait_for_tasks() {
	match TASKS.try_write_for(Duration::from_secs(5)) {
		Some(_) => (),
		None => panic!(
			"Threads failed to release resources within 5 seconds, this may indicate a deadlock!"
		),
	}
}
#[byondapi::init]
pub fn initialize_turfs() {
	// 10x 255x255 zlevels
	// double that for edges since each turf can have up to 6 edges but eehhhh
	*TURF_GASES.write() = Some(TurfGases {
		graph: StableDiGraph::with_capacity(650_250, 1_300_500),
		map: IndexMap::with_capacity_and_hasher(650_250, FxBuildHasher),
	});
	*PLANETARY_ATMOS.write() = Some(Default::default());
}

pub fn shutdown_turfs() {
	wait_for_tasks();
	TURF_GASES.write().as_mut().unwrap().clear();
	PLANETARY_ATMOS.write().as_mut().unwrap().clear();
}

fn with_turf_gases_read<T, F>(f: F) -> T
where
	F: FnOnce(&TurfGases) -> T,
{
	f(TURF_GASES.read().as_ref().unwrap())
}

fn with_turf_gases_write<T, F>(f: F) -> T
where
	F: FnOnce(&mut TurfGases) -> T,
{
	f(TURF_GASES.write().as_mut().unwrap())
}

fn with_planetary_atmos<T, F>(f: F) -> T
where
	F: FnOnce(&IndexMap<u32, Mixture, FxBuildHasher>) -> T,
{
	f(PLANETARY_ATMOS.read().as_ref().unwrap())
}

fn with_planetary_atmos_upgradeable_read<T, F>(f: F) -> T
where
	F: FnOnce(RwLockUpgradableReadGuard<'_, Option<IndexMap<u32, Mixture, FxBuildHasher>>>) -> T,
{
	f(PLANETARY_ATMOS.upgradable_read())
}

/// Returns: null. Updates turf air infos, whether the turf is closed, is space or a regular turf, or even a planet turf is decided here.
#[byondapi::bind("/turf/proc/update_air_ref")]
fn hook_register_turf(src: ByondValue, flag: ByondValue) -> Result<ByondValue> {
	let id = src.get_ref()?;
	let flag = flag.get_number()? as i32;
	if let Ok(blocks) = src.read_number_id(byond_string!("blocks_air")) {
		if blocks > 0.0 {
			with_turf_gases_write(|arena| arena.remove_turf(id));
			#[cfg(feature = "superconductivity")]
			superconduct::supercond_update_ref(src)?;
			return Ok(ByondValue::null());
		}
	}
	if flag >= 0 {
		let mut to_insert: TurfMixture = TurfMixture::default();
		let air = src.read_var_id(byond_string!("air"))?;
		to_insert.mix = air.read_number_id(byond_string!("_extools_pointer_gasmixture"))? as usize;
		to_insert.flags = SimulationFlags::from_bits_truncate(flag as u8);
		to_insert.id = id;

		if let Ok(is_planet) = src.read_number_id(byond_string!("planetary_atmos")) {
			if is_planet != 0.0 {
				if let Ok(at_str) = src.read_string_id(byond_string!("initial_gas_mix")) {
					with_planetary_atmos_upgradeable_read(|lock| {
						to_insert.planetary_atmos = Some({
							let mut state = rustc_hash::FxHasher::default();
							at_str.hash(&mut state);
							state.finish() as u32
						});
						if lock
							.as_ref()
							.unwrap()
							.contains_key(&to_insert.planetary_atmos.unwrap())
						{
							return;
						}

						let mut write =
							parking_lot::lock_api::RwLockUpgradableReadGuard::upgrade(lock);

						write
							.as_mut()
							.unwrap()
							.insert(to_insert.planetary_atmos.unwrap(), {
								let mut gas = to_insert.get_gas_copy();
								gas.mark_immutable();
								gas
							});
					});
				}
			}
		}
		with_turf_gases_write(|arena| arena.insert_turf(to_insert));
	} else {
		with_turf_gases_write(|arena| arena.remove_turf(id));
	}

	#[cfg(feature = "superconductivity")]
	superconduct::supercond_update_ref(src)?;
	Ok(ByondValue::null())
}

/* will come back to you later
const PLANET_TURF: i32 = 1;
const SPACE_TURF: i32 = 0;
const CLOSED_TURF: i32 = -1;
const OPEN_TURF: i32 = 2;

//hardcoded because we can't have nice things
fn determine_turf_flag(src: &ByondValue) -> i32 {
	let path = src
		.read_string_id(byond_string!("("type")
		.unwrap_or_else(|_| "TYPPENOTFOUND".to_string());
	if !path.as_str().starts_with("/turf/open") {
		CLOSED_TURF
	} else if src.read_number_id(byond_string!("planetary_atmos")).unwrap_or(0.0) > 0.0 {
		PLANET_TURF
	} else if path.as_str().starts_with("/turf/open/space") {
		SPACE_TURF
	} else {
		OPEN_TURF
	}
}
*/
/// Updates adjacency infos for turfs, only use this in immediateupdateturfs.
#[byondapi::bind("/turf/proc/__update_auxtools_turf_adjacency_info")]
fn hook_infos(src: ByondValue) -> Result<ByondValue> {
	let id = src.get_ref()?;
	with_turf_gases_write(|arena| -> Result<()> {
		if let Some(adjacent_list) = src
			.read_var_id(byond_string!("atmos_adjacent_turfs"))
			.ok()
			.and_then(|adjs| adjs.is_list().then_some(adjs))
		{
			arena.update_adjacencies(id, adjacent_list)?;
		} else if let Some(&idx) = arena.map.get(&id) {
			arena.remove_adjacencies(idx);
		}
		Ok(())
	})?;

	#[cfg(feature = "superconductivity")]
	superconduct::supercond_update_adjacencies(id)?;
	Ok(ByondValue::null())
}

/// Updates the visual overlays for the given turf.
/// Will use a cached overlay list if one exists.
/// # Errors
/// If auxgm wasn't implemented properly or there's an invalid gas mixture.
fn update_visuals(src: ByondValue) -> Result<ByondValue> {
	use super::gas;
	match src.read_var_id(byond_string!("air")) {
		Err(_) => Ok(ByondValue::null()),
		Ok(air) => {
			// gas_overlays: list( GAS_ID = list( VIS_FACTORS = OVERLAYS )) got it? I don't
			let gas_overlays = ByondValue::new_global_ref()
				.read_var_id(byond_string!("GLOB"))
				.wrap_err("Unable to get GLOB from BYOND globals")?
				.read_var_id(byond_string!("gas_data"))
				.wrap_err("gas_data is undefined on GLOB")?
				.read_var_id(byond_string!("overlays"))
				.wrap_err("overlays is undefined in GLOB.gas_data")?;
			let ptr = air
				.read_var_id(byond_string!("_extools_pointer_gasmixture"))
				.wrap_err("air is undefined on turf")?
				.get_number()
				.wrap_err("Gas mixture has invalid pointer")? as usize;
			let overlay_types = GasArena::with_gas_mixture(ptr, |mix| {
				Ok(mix
					.enumerate()
					.filter_map(|(idx, moles)| Some((idx, moles, gas::types::gas_visibility(idx)?)))
					.filter(|(_, moles, amt)| moles > amt)
					// getting the list(VIS_FACTORS = OVERLAYS) with GAS_ID
					.filter_map(|(idx, moles, _)| {
						Some((
							gas_overlays.read_list_index(gas::gas_idx_to_id(idx)).ok()?,
							moles,
						))
					})
					// getting the OVERLAYS with VIS_FACTOR
					.filter_map(|(this_overlay_list, moles)| {
						this_overlay_list
							.read_list_index(gas::mixture::visibility_step(moles) as f32)
							.ok()
					})
					.collect::<Vec<_>>())
			})?;

			Ok(src
				.call_id(
					byond_string!("set_visuals"),
					&[overlay_types.as_slice().try_into()?],
				)
				.wrap_err("Calling set_visuals")?)
		}
	}
}

const fn adjacent_tile_id(id: u8, i: TurfID, max_x: i32, max_y: i32) -> TurfID {
	let z_size = max_x * max_y;
	let i = i as i32;
	match id {
		0 => (i + max_x) as TurfID,
		1 => (i - max_x) as TurfID,
		2 => (i + 1) as TurfID,
		3 => (i - 1) as TurfID,
		4 => (i + z_size) as TurfID,
		5 => (i - z_size) as TurfID,
		_ => panic!("Invalid id passed to adjacent_tile_id!"),
	}
}

#[derive(Clone, Copy)]
struct AdjacentTileIDs {
	adj: Directions,
	i: TurfID,
	max_x: i32,
	max_y: i32,
	count: u8,
}

impl Iterator for AdjacentTileIDs {
	type Item = (Directions, TurfID);

	fn next(&mut self) -> Option<Self::Item> {
		loop {
			if self.count == 6 {
				return None;
			}
			//SAFETY: count can never be invalid
			let dir = Directions::from_bits_retain(1 << self.count);
			self.count += 1;
			if self.adj.contains(dir) {
				return Some((
					dir,
					adjacent_tile_id(self.count - 1, self.i, self.max_x, self.max_y),
				));
			}
		}
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		(0, Some(self.adj.bits().count_ones() as usize))
	}
}

use std::iter::FusedIterator;

impl FusedIterator for AdjacentTileIDs {}

#[allow(unused)]
fn adjacent_tile_ids(adj: Directions, i: TurfID, max_x: i32, max_y: i32) -> AdjacentTileIDs {
	AdjacentTileIDs {
		adj,
		i,
		max_x,
		max_y,
		count: 0,
	}
}
