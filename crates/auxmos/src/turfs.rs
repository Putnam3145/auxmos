mod processing;

#[cfg(feature = "monstermos")]
mod monstermos;

#[cfg(feature = "putnamos")]
mod putnamos;

#[cfg(feature = "katmos")]
mod katmos;

#[cfg(feature = "superconductivity")]
mod superconduct;

use auxtools::*;

use rayon::prelude::*;

use crate::{constants::*, gas::Mixture, GasArena};

use dashmap::DashMap;

use fxhash::FxBuildHasher;

use bitflags::bitflags;

use parking_lot::{const_mutex, const_rwlock, Mutex, RwLock};

use petgraph::{graph::NodeIndex, stable_graph::StableDiGraph, visit::EdgeRef, Direction};

use indexmap::IndexMap;

use std::{mem::drop, sync::atomic::AtomicU64};

bitflags! {
	#[derive(Default)]
	pub struct Directions: u8 {
		const NORTH = 0b1;
		const SOUTH = 0b10;
		const EAST	= 0b100;
		const WEST	= 0b1000;
		const UP 	= 0b10000;
		const DOWN 	= 0b100000;
		const ALL_CARDINALS = Self::NORTH.bits | Self::SOUTH.bits | Self::EAST.bits | Self::WEST.bits;
		const ALL_CARDINALS_MULTIZ = Self::NORTH.bits | Self::SOUTH.bits | Self::EAST.bits | Self::WEST.bits | Self::UP.bits | Self::DOWN.bits;
	}

	#[derive(Default)]
	pub struct SimulationFlags: u8 {
		const SIMULATION_DIFFUSE = 0b1;
		const SIMULATION_ALL = 0b10;
		const SIMULATION_ANY = Self::SIMULATION_DIFFUSE.bits | Self::SIMULATION_ALL.bits;
	}

	#[derive(Default)]
	pub struct AdjacentFlags: u8 {
		const ATMOS_ADJACENT_ANY = 0b1;
		const ATMOS_ADJACENT_FIRELOCK = 0b10;
	}

	#[derive(Default)]
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
#[derive(Default)]
struct TurfMixture {
	pub mix: usize,
	pub id: TurfID,
	pub flags: SimulationFlags,
	pub planetary_atmos: Option<u32>,
	pub vis_hash: AtomicU64,
}

#[allow(dead_code)]
impl TurfMixture {
	pub fn enabled(&self) -> bool {
		self.flags.intersects(SimulationFlags::SIMULATION_ANY)
	}

	pub fn is_immutable(&self) -> bool {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.is_immutable()
		})
	}
	pub fn return_pressure(&self) -> f32 {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.return_pressure()
		})
	}
	pub fn return_temperature(&self) -> f32 {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.get_temperature()
		})
	}
	pub fn total_moles(&self) -> f32 {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.total_moles()
		})
	}
	pub fn clear_air(&self) {
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.unwrap_or_else(|| panic!("Gas mixture not found for turf: {}", self.mix))
				.write()
				.clear();
		});
	}
	pub fn clear_vol(&self, amt: f32) {
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
	pub fn invalidate_vis_cache(&self) {
		self.vis_hash.store(0, std::sync::atomic::Ordering::Relaxed);
	}
}

type TurfGraphMap = IndexMap<TurfID, NodeIndex<usize>, FxBuildHasher>;

//adjacency/turf infos goes here
struct TurfGases {
	graph: StableDiGraph<TurfMixture, AdjacentFlags, usize>,
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
		if let Some(index) = self.map.remove(&id) {
			self.graph.remove_node(index);
		}
	}
	/*
	pub fn invalidate(&mut self) {
		*self.map.lock() = None;
	}
	*/
	/*
	pub fn turf_id_map(&self) -> TurfGraphMap {
		self.map
			.lock()
			.get_or_insert_with(|| {
				self.graph.
					.enumerate()
					.map(|(i, n)| (n.weight.id, NodeIndex::from(i)))
					.collect()
			})
			.clone()
	}
	*/
	pub fn update_adjacencies(&mut self, idx: TurfID, adjacent_list: List) -> Result<(), Runtime> {
		if let Some(&this_index) = self.map.get(&idx) {
			self.remove_adjacencies(this_index);
			for i in 1..=adjacent_list.len() {
				let adj_val = adjacent_list.get(i)?;
				//let adjacent_num = adjacent_list.get(&adj_val)?.as_number()? as u8;
				if let Some(&adj_index) = self.map.get(&unsafe { adj_val.raw.data.id }) {
					let flags = AdjacentFlags::from_bits_truncate(
						adjacent_list
							.get(adj_val)
							.and_then(|g| g.as_number())
							.unwrap_or(0.0) as u8,
					);
					if flags.contains(AdjacentFlags::ATMOS_ADJACENT_ANY) {
						self.graph.add_edge(this_index, adj_index, flags);
					}
				}
			}
		};
		Ok(())
	}

	//This isn't a useless collect(), we can't hold a mutable ref and an immutable ref at once on the graph
	#[allow(clippy::needless_collect)]
	pub fn remove_adjacencies(&mut self, index: NodeIndex<usize>) {
		let edges = self
			.graph
			.edges(index)
			.map(|edgeref| edgeref.id())
			.collect::<Vec<_>>();
		edges.into_iter().for_each(|edgeindex| {
			self.graph.remove_edge(edgeindex);
		});
	}

	pub fn get(&self, idx: NodeIndex<usize>) -> Option<&TurfMixture> {
		self.graph.node_weight(idx)
	}

	pub fn get_id(&self, idx: &TurfID) -> Option<&NodeIndex<usize>> {
		self.map.get(idx)
	}

	pub fn adjacent_node_ids<'a>(
		&'a self,
		index: NodeIndex<usize>,
	) -> impl Iterator<Item = NodeIndex<usize>> + '_ {
		self.graph.neighbors(index)
	}

	#[allow(unused)]
	pub fn adjacent_turf_ids<'a>(
		&'a self,
		index: NodeIndex<usize>,
	) -> impl Iterator<Item = TurfID> + '_ {
		self.graph
			.neighbors(index)
			.filter_map(|index| Some(self.get(index)?.id))
	}

	#[allow(unused)]
	pub fn adjacent_node_ids_enabled<'a>(
		&'a self,
		index: NodeIndex<usize>,
	) -> impl Iterator<Item = NodeIndex<usize>> + '_ {
		self.graph.neighbors(index).filter(|&adj_index| {
			self.graph
				.node_weight(adj_index)
				.map_or(false, |mix| mix.enabled())
		})
	}

	pub fn adjacent_mixes<'a>(
		&'a self,
		index: NodeIndex<usize>,
		all_mixtures: &'a [parking_lot::RwLock<Mixture>],
	) -> impl Iterator<Item = &'a parking_lot::RwLock<Mixture>> {
		self.graph
			.neighbors(index)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
			.filter_map(move |idx| all_mixtures.get(idx.mix))
	}

	pub fn adjacent_mixes_with_adj_ids<'a>(
		&'a self,
		index: NodeIndex<usize>,
		all_mixtures: &'a [parking_lot::RwLock<Mixture>],
		dir: Direction,
	) -> impl Iterator<Item = (&'a TurfID, &'a parking_lot::RwLock<Mixture>)> {
		self.graph
			.neighbors_directed(index, dir)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
			.filter_map(move |idx| Some((&idx.id, all_mixtures.get(idx.mix)?)))
	}

	/*
	pub fn adjacent_infos(
		&self,
		index: NodeIndex<usize>,
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
static mut PLANETARY_ATMOS: Option<DashMap<u32, Mixture, FxBuildHasher>> = None;

//whether there is any tasks running
static TASKS: RwLock<()> = const_rwlock(());

// Turfs need updating before the thread starts
static DIRTY_TURFS: Mutex<Option<IndexMap<TurfID, DirtyFlags, FxBuildHasher>>> = const_mutex(None);

static ANY_TURF_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

//block until threads are done
pub fn wait_for_tasks() {
	drop(TASKS.write())
}

#[init(partial)]
fn _initialize_turf_statics() -> Result<(), String> {
	unsafe {
		// 10x 255x255 zlevels
		// double that for edges since each turf can have up to 6 edges but eehhhh
		*TURF_GASES.write() = Some(TurfGases {
			graph: StableDiGraph::with_capacity(650_250, 1_300_500),
			map: IndexMap::with_capacity_and_hasher(650_250, FxBuildHasher::default()),
		});
		PLANETARY_ATMOS = Some(Default::default());
		*DIRTY_TURFS.lock() = Some(Default::default());
	};
	Ok(())
}

#[shutdown]
fn _shutdown_turfs() {
	wait_for_tasks();
	*DIRTY_TURFS.lock() = None;
	*TURF_GASES.write() = None;
	unsafe {
		PLANETARY_ATMOS = None;
	}
}

fn set_turfs_dirty(b: bool) {
	ANY_TURF_DIRTY.store(b, std::sync::atomic::Ordering::Relaxed);
}

fn check_turfs_dirty() -> bool {
	ANY_TURF_DIRTY.load(std::sync::atomic::Ordering::Relaxed)
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

fn with_dirty_turfs<T, F>(f: F) -> T
where
	F: FnOnce(&mut IndexMap<u32, DirtyFlags, FxBuildHasher>) -> T,
{
	set_turfs_dirty(true);
	f(DIRTY_TURFS.lock().as_mut().unwrap())
}

fn planetary_atmos() -> &'static DashMap<u32, Mixture, FxBuildHasher> {
	unsafe { PLANETARY_ATMOS.as_ref().unwrap() }
}

fn rebuild_turf_graph() -> Result<(), Runtime> {
	with_dirty_turfs(|dirty_turfs| {
		for (&t, _) in dirty_turfs
			.iter()
			.filter(|&(_, &flags)| flags.contains(DirtyFlags::DIRTY_MIX_REF))
		{
			register_turf(t)?;
		}
		for (t, _) in dirty_turfs
			.drain(..)
			.filter(|&(_, flags)| flags.contains(DirtyFlags::DIRTY_ADJACENT))
		{
			update_adjacency_info(t)?;
		}
		Ok(())
	})?;
	set_turfs_dirty(false);
	Ok(())
}

fn register_turf(id: u32) -> Result<(), Runtime> {
	let src = unsafe { Value::turf_by_id_unchecked(id) };
	let flag = determine_turf_flag(&src);
	if flag >= 0 {
		let mut to_insert: TurfMixture = TurfMixture::default();
		let air = src.get(byond_string!("air"))?;
		to_insert.mix = air
			.get_number(byond_string!("_extools_pointer_gasmixture"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?
			.to_bits() as usize;
		to_insert.flags = SimulationFlags::from_bits_truncate(flag as u8);
		to_insert.id = id;

		if let Ok(is_planet) = src.get_number(byond_string!("planetary_atmos")) {
			if is_planet != 0.0 {
				if let Ok(at_str) = src.get_string(byond_string!("initial_gas_mix")) {
					to_insert.planetary_atmos = Some(fxhash::hash32(&at_str));
					let mut entry = planetary_atmos()
						.entry(to_insert.planetary_atmos.unwrap())
						.or_insert_with(|| {
							let mut gas = to_insert.get_gas_copy();
							gas.mark_immutable();
							gas
						});
					entry.mark_immutable();
				}
			}
		}
		with_turf_gases_write(|arena| arena.insert_turf(to_insert));
	} else {
		with_turf_gases_write(|arena| arena.remove_turf(id));
	}

	#[cfg(feature = "superconductivity")]
	superconduct::supercond_update_ref(src)?;

	Ok(())
}

#[hook("/turf/proc/update_air_ref")]
fn _hook_register_turf() {
	with_dirty_turfs(|dirty_turfs| {
		dirty_turfs
			.entry(unsafe { src.raw.data.id })
			.or_default()
			.insert(DirtyFlags::DIRTY_MIX_REF);
	});
	Ok(Value::null())
}

const PLANET_TURF: i32 = 1;
const SPACE_TURF: i32 = 0;
const CLOSED_TURF: i32 = -1;
const OPEN_TURF: i32 = 2;

//hardcoded because we can't have nice things
fn determine_turf_flag(src: &Value) -> i32 {
	let path = src
		.get_type()
		.unwrap_or_else(|_| "TYPPENOTFOUND".to_string());
	if !path.as_str().starts_with("/turf/open") {
		CLOSED_TURF
	} else if src
		.get_number(byond_string!("planetary_atmos"))
		.unwrap_or(0.0)
		> 0.0
	{
		PLANET_TURF
	} else if path.as_str().starts_with("/turf/open/space") {
		SPACE_TURF
	} else {
		OPEN_TURF
	}
}

fn update_adjacency_info(id: u32) -> Result<(), Runtime> {
	let src_turf = unsafe { Value::turf_by_id_unchecked(id) };
	with_turf_gases_write(|arena| -> Result<(), Runtime> {
		if let Ok(adjacent_list) = src_turf.get_list(byond_string!("atmos_adjacent_turfs")) {
			arena.update_adjacencies(id, adjacent_list)?;
		} else if let Some(&idx) = arena.map.get(&id) {
			arena.remove_adjacencies(idx);
		}
		Ok(())
	})?;

	#[cfg(feature = "superconductivity")]
	superconduct::supercond_update_adjacencies(id)?;
	Ok(())
}

#[hook("/turf/proc/__update_auxtools_turf_adjacency_info")]
fn _hook_infos() {
	with_dirty_turfs(|dirty_turfs| -> Result<(), Runtime> {
		let e = dirty_turfs.entry(unsafe { src.raw.data.id }).or_default();
		e.insert(DirtyFlags::DIRTY_ADJACENT);
		Ok(())
	})?;
	Ok(Value::null())
}

// gas_overlays: list( GAS_ID = list( VIS_FACTORS = OVERLAYS )) got it? I don't
/// Updates the visual overlays for the given turf.
/// Will use a cached overlay list if one exists.
/// # Errors
/// If auxgm wasn't implemented properly or there's an invalid gas mixture.
fn update_visuals(src: Value) -> DMResult {
	use super::gas;
	match src.get(byond_string!("air")) {
		Err(_) => Ok(Value::null()),
		Ok(air) => {
			let overlay_types = List::new();
			let gas_overlays = Value::globals()
				.get(byond_string!("gas_data"))?
				.get_list(byond_string!("overlays"))?;
			let ptr = air
				.get_number(byond_string!("_extools_pointer_gasmixture"))
				.map_err(|_| {
					runtime!(
						"Attempt to interpret non-number value as number {} {}:{}",
						std::file!(),
						std::line!(),
						std::column!()
					)
				})?
				.to_bits() as usize;
			GasArena::with_gas_mixture(ptr, |mix| {
				mix.for_each_gas(|idx, moles| {
					if let Some(amt) = gas::types::gas_visibility(idx) {
						if moles > amt {
							let this_overlay_list =
								gas_overlays.get(gas::gas_idx_to_id(idx)?)?.as_list()?;
							if let Ok(this_gas_overlay) =
								this_overlay_list.get(gas::mixture::visibility_step(moles))
							{
								overlay_types.append(this_gas_overlay);
							}
						}
					}
					Ok(())
				})
			})?;

			src.call("set_visuals", &[&Value::from(overlay_types)])
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
			let dir = unsafe { Directions::from_bits_unchecked(1 << self.count) };
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
