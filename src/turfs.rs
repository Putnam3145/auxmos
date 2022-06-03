pub mod processing;

#[cfg(feature = "monstermos")]
pub mod monstermos;

#[cfg(feature = "putnamos")]
pub mod putnamos;

#[cfg(feature = "katmos")]
pub mod katmos;

use crate::gas::Mixture;

use auxtools::*;

use crate::constants::*;

use crate::GasArena;

use dashmap::DashMap;

use fxhash::FxBuildHasher;

use rayon;

use rayon::prelude::*;

use std::collections::HashMap;
use std::mem::drop;

use crate::callbacks::aux_callbacks_sender;

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use petgraph::{graph::NodeIndex, stable_graph::StableDiGraph, visit::EdgeRef};

const NORTH: u8 = 1;
const SOUTH: u8 = 2;
const EAST: u8 = 4;
const WEST: u8 = 8;
const UP: u8 = 16;
const DOWN: u8 = 32;

#[allow(unused)]
const fn adj_flag_to_idx(adj_flag: u8) -> usize {
	match adj_flag {
		NORTH => 0,
		SOUTH => 1,
		EAST => 2,
		WEST => 3,
		UP => 4,
		DOWN => 5,
		_ => 6,
	}
}

pub const NONE: u8 = 0;
pub const SIMULATION_DIFFUSE: u8 = 1;
pub const SIMULATION_ALL: u8 = 2;
pub const SIMULATION_ANY: u8 = SIMULATION_DIFFUSE | SIMULATION_ALL;
pub const SIMULATION_DISABLED: u8 = 4;
pub const SIMULATION_FLAGS: u8 = SIMULATION_ANY | SIMULATION_DISABLED;

type TurfID = u32;

// TurfMixture can be treated as "immutable" for all intents and purposes--put other data somewhere else
#[derive(Clone, Copy, Default, Hash, Eq, PartialEq)]
struct TurfMixture {
	pub mix: usize,
	pub id: TurfID,
	pub flags: u8,
	pub planetary_atmos: Option<u32>,
}

#[allow(dead_code)]
impl TurfMixture {
	pub fn enabled(&self) -> bool {
		let simul_flags = self.flags & SIMULATION_FLAGS;
		simul_flags & SIMULATION_DISABLED == 0 && simul_flags & SIMULATION_ANY != 0
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
}

// all non-space turfs get these, not just ones with air--a lot of gas logic relies on all TurfMixtures having a valid mix
#[derive(Clone, Copy, Default)]
struct ThermalInfo {
	pub temperature: f32,
	pub thermal_conductivity: f32,
	pub heat_capacity: f32,
	pub adjacency: u8,
	pub adjacent_to_space: bool,
}
struct TurfGases {
	graph: StableDiGraph<TurfMixture, ()>,
	map: HashMap<TurfID, NodeIndex, FxBuildHasher>,
}

impl TurfGases {
	pub fn remove_turf(&mut self, idx: TurfID) {
		if let Some(tmix) = self.map.remove(&idx) {
			self.graph.remove_node(tmix);
		}
	}

	pub fn insert_turf(&mut self, idx: TurfID, tmix: TurfMixture) {
		self.map.insert(idx, self.graph.add_node(tmix));
	}

	pub fn disable_turf(&mut self, idx: TurfID) {
		if let Some(&index) = self.map.get(&idx) {
			if let Some(node) = self.graph.node_weight_mut(index) {
				node.flags |= SIMULATION_DISABLED
			}
		}
	}

	pub fn enable_turf(&mut self, idx: TurfID) {
		if let Some(&index) = self.map.get(&idx) {
			if let Some(node) = self.graph.node_weight_mut(index) {
				node.flags &= !SIMULATION_DISABLED
			}
		}
	}

	pub fn update_adjacencies(&mut self, idx: TurfID, adjacent_list: List) -> Result<(), Runtime> {
		if let Some(&this_index) = self.map.get(&idx) {
			self.remove_adjacencies(idx);
			for i in 1..=adjacent_list.len() {
				let adj_val = adjacent_list.get(i)?;
				//let adjacent_num = adjacent_list.get(&adj_val)?.as_number()? as u8;
				if let Some(&adj_index) = self.map.get(&unsafe { adj_val.raw.data.id }) {
					self.graph.add_edge(this_index, adj_index, ());
				}
			}
		};
		Ok(())
	}

	//This isn't a useless collect(), we can't hold a mutable ref and an immutable ref at once on the graph
	pub fn remove_adjacencies(&mut self, idx: TurfID) {
		if let Some(index) = self.map.get(&idx) {
			let edges = self
				.graph
				.edges(*index)
				.map(|edgeref| edgeref.id())
				.collect::<Vec<_>>();
			edges.into_iter().for_each(|edgeindex| {
				self.graph.remove_edge(edgeindex);
			});
		}
	}

	pub fn get_from_turfid(&self, idx: &TurfID) -> Option<&TurfMixture> {
		self.map
			.get(idx)
			.and_then(|index| self.graph.node_weight(*index))
	}

	pub fn get_nodeid(&self, idx: &TurfID) -> Option<NodeIndex> {
		self.map.get(idx).copied()
	}

	pub fn get(&self, idx: NodeIndex) -> Option<&TurfMixture> {
		self.graph.node_weight(idx)
	}

	pub fn adjacent_node_ids<'a>(
		&'a self,
		index: NodeIndex,
	) -> impl Iterator<Item = NodeIndex> + '_ {
		self.graph.neighbors(index)
	}

	pub fn adjacent_turf_ids<'a>(&'a self, index: NodeIndex) -> impl Iterator<Item = TurfID> + '_ {
		self.graph
			.neighbors(index)
			.filter_map(|index| Some(self.get(index)?.id))
	}

	pub fn adjacent_node_ids_enabled<'a>(
		&'a self,
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
	) -> impl Iterator<Item = (&'a TurfID, &'a parking_lot::RwLock<Mixture>)> {
		self.graph
			.neighbors(index)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
			.filter_map(move |idx| Some((&idx.id, all_mixtures.get(idx.mix)?)))
	}

	pub fn adjacent_infos(&self, index: NodeIndex) -> impl Iterator<Item = &TurfMixture> {
		self.graph
			.neighbors(index)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
	}

	/*
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

static mut TURF_GASES: Option<RwLock<TurfGases>> = None;
// Turfs with temperatures/heat capacities. This is distinct from the above.
static mut TURF_TEMPERATURES: Option<DashMap<TurfID, ThermalInfo, FxBuildHasher>> = None;
// We store planetary atmos by hash of the initial atmos string here for speed.
static mut PLANETARY_ATMOS: Option<DashMap<u32, Mixture, FxBuildHasher>> = None;

#[init(partial)]
fn _initialize_turf_statics() -> Result<(), String> {
	unsafe {
		// 10x 255x255 zlevels
		// double that for edges since each turf can have up to 6 edges but eehhhh
		TURF_GASES = Some(RwLock::new(TurfGases {
			map: HashMap::with_capacity_and_hasher(650_250, FxBuildHasher::default()),
			graph: StableDiGraph::with_capacity(650_250, 1_300_500),
		}));
		TURF_TEMPERATURES = Some(Default::default());
		PLANETARY_ATMOS = Some(Default::default());
	};
	Ok(())
}

#[shutdown]
fn _shutdown_turfs() {
	unsafe {
		TURF_GASES = None;
		TURF_TEMPERATURES = None;
		PLANETARY_ATMOS = None;
	}
}

// this would lead to undefined info if it were possible for something to put a None on it during operation, but nothing's going to do that
fn with_turf_gases_read<T, F>(f: F) -> T
where
	F: FnOnce(RwLockReadGuard<TurfGases>) -> T,
{
	f(unsafe { TURF_GASES.as_ref().unwrap().read() })
}

fn with_turf_gases_write<T, F>(f: F) -> T
where
	F: FnOnce(RwLockWriteGuard<TurfGases>) -> T,
{
	f(unsafe { TURF_GASES.as_ref().unwrap().write() })
}

fn planetary_atmos() -> &'static DashMap<u32, Mixture, FxBuildHasher> {
	unsafe { PLANETARY_ATMOS.as_ref().unwrap() }
}

fn turf_temperatures() -> &'static DashMap<TurfID, ThermalInfo, FxBuildHasher> {
	unsafe { TURF_TEMPERATURES.as_ref().unwrap() }
}

#[hook("/turf/proc/update_air_ref")]
fn _hook_register_turf() {
	let sender = aux_callbacks_sender(crate::callbacks::TURFS);
	let id = unsafe { src.raw.data.id };
	drop(sender.send(Box::new(move || {
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
			to_insert.flags = flag as u8;
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
			with_turf_gases_write(|mut arena| arena.insert_turf(id, to_insert));
		} else {
			with_turf_gases_write(|mut arena| arena.remove_turf(id));
		}
		Ok(Value::null())
	})));
	Ok(Value::null())
}

const PLANET_TURF: i32 = 1;
const SPACE_TURF: i32 = 0;
const CLOSED_TURF: i32 = -1;
const OPEN_TURF: i32 = 2;

//hardcoded because we can't have nice things
fn determine_turf_flag(src: &Value) -> i32 {
	let path = src.get_type().unwrap_or_else(|_| "TYPPENOTFOUND".to_string());
	let is_open = path.as_str().starts_with("/turf/open");
	let is_planet = src
		.get_number(byond_string!("planetary_atmos"))
		.unwrap_or(0.0)
		> 0.0;
	let is_space = path.as_str().starts_with("/turf/open/space");

	let mut flag: i32 = CLOSED_TURF;
	if is_space {
		flag = SPACE_TURF
	} else if is_planet && is_open {
		flag = PLANET_TURF
	} else if is_open {
		flag = OPEN_TURF
	}
	flag
}

#[hook("/turf/proc/__auxtools_update_turf_temp_info")]
fn _hook_turf_update_temp() {
	let sender = aux_callbacks_sender(crate::callbacks::TEMPERATURE);
	let id = unsafe { src.raw.data.id };
	if src
		.get_number(byond_string!("thermal_conductivity"))
		.unwrap_or_default()
		> 0.0 && src
		.get_number(byond_string!("heat_capacity"))
		.unwrap_or_default()
		> 0.0
	{
		let mut to_insert = ThermalInfo {
			temperature: 293.15,
			thermal_conductivity: 0.0,
			heat_capacity: 0.0,
			adjacency: NORTH | SOUTH | WEST | EAST,
			adjacent_to_space: false,
		};
		to_insert.thermal_conductivity = src
			.get_number(byond_string!("thermal_conductivity"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?;
		to_insert.heat_capacity = src
			.get_number(byond_string!("heat_capacity"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?;
		to_insert.adjacency = NORTH | SOUTH | WEST | EAST;
		to_insert.adjacent_to_space = args[0].as_number().map_err(|_| {
			runtime!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})? != 0.0;
		to_insert.temperature = src
			.get_number(byond_string!("initial_temperature"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?;
		drop(sender.send(Box::new(move || {
			turf_temperatures().insert(id, to_insert);
			Ok(Value::null())
		})));
	} else {
		drop(sender.send(Box::new(move || {
			turf_temperatures().remove(&id);
			Ok(Value::null())
		})));
	}
	Ok(Value::null())
}
/* will deadlock, don't recommend using this
#[hook("/turf/proc/set_sleeping")]
fn _hook_sleep() {
	let arg = if let Some(arg_get) = args.get(0) {
		// null is falsey in byond so
		arg_get.as_number().map_err(|_| {
			runtime!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})?
	} else {
		0.0
	};
	let src_id = unsafe { src.raw.data.id };
	if arg == 0.0 {
		turf_gases().enable_turf(src_id);
	} else {
		turf_gases().disable_turf(src_id);
	}
	Ok(Value::from(true))
}
*/

#[hook("/turf/proc/__update_auxtools_turf_adjacency_info")]
fn _hook_infos(arg0: Value, arg1: Value) {
	let update_now = arg1.as_number().unwrap_or(0.0) != 0.0;
	let adjacent_to_spess = arg0.as_number().unwrap_or(0.0) != 0.0;
	let id = unsafe { src.raw.data.id };
	let sender = aux_callbacks_sender(crate::callbacks::ADJACENCIES);
	let boxed_fn: Box<dyn Fn() -> DMResult + Send + Sync> = Box::new(move || {
		let src_turf = unsafe { Value::turf_by_id_unchecked(id) };
		with_turf_gases_write(|mut arena| -> Result<(), Runtime> {
			if let Ok(adjacent_list) = src_turf.get_list(byond_string!("atmos_adjacent_turfs")) {
				arena.update_adjacencies(id, adjacent_list)?;
			} else {
				arena.remove_adjacencies(id);
			}
			if let Ok(blocks_air) = src_turf.get_number(byond_string!("blocks_air")) {
				if blocks_air == 0.0 {
					arena.enable_turf(id)
				} else {
					arena.disable_turf(id)
				}
			}
			Ok(())
		})?;
		if let Ok(atmos_blocked_directions) =
			src_turf.get_number(byond_string!("conductivity_blocked_directions"))
		{
			let adjacency = NORTH | SOUTH | WEST | EAST & !(atmos_blocked_directions as u8);
			turf_temperatures()
				.entry(id)
				.and_modify(|entry| {
					entry.adjacency = adjacency;
				})
				.or_insert_with(|| ThermalInfo {
					temperature: src_turf
						.get_number(byond_string!("initial_temperature"))
						.unwrap_or(TCMB),
					thermal_conductivity: src_turf
						.get_number(byond_string!("thermal_conductivity"))
						.unwrap_or(0.0),
					heat_capacity: src_turf
						.get_number(byond_string!("heat_capacity"))
						.unwrap_or(0.0),
					adjacency,
					adjacent_to_space: adjacent_to_spess,
				});
		}
		Ok(Value::null())
	});
	if update_now {
		boxed_fn()?;
	} else {
		drop(sender.send(boxed_fn));
	}
	Ok(Value::null())
}

#[hook("/turf/proc/return_temperature")]
fn _hook_turf_temperature() {
	if let Some(temp_info) = turf_temperatures().get(&unsafe { src.raw.data.id }) {
		if temp_info.temperature.is_normal() {
			Ok(Value::from(temp_info.temperature))
		} else {
			src.get(byond_string!("initial_temperature"))
		}
	} else {
		src.get(byond_string!("initial_temperature"))
	}
}

// gas_overlays: list( GAS_ID = list( VIS_FACTORS = OVERLAYS )) got it? I don't
/// Updates the visual overlays for the given turf.
/// Will use a cached overlay list if one exists.
/// # Errors
/// If auxgm wasn't implemented properly or there's an invalid gas mixture.
pub fn update_visuals(src: Value) -> DMResult {
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
							if let Ok(this_gas_overlay) = this_overlay_list.get(
								(moles / MOLES_GAS_VISIBLE_STEP)
									.ceil()
									.min(FACTOR_GAS_VISIBLE_MAX)
									.max(1.0) as u32,
							) {
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

fn adjacent_tile_id(id: u8, i: TurfID, max_x: i32, max_y: i32) -> TurfID {
	let z_size = max_x * max_y;
	let i = i as i32;
	match id {
		0 => (i + max_x) as TurfID,
		1 => (i - max_x) as TurfID,
		2 => (i + 1) as TurfID,
		3 => (i - 1) as TurfID,
		4 => (i + z_size) as TurfID,
		5 => (i - z_size) as TurfID,
		_ => i as TurfID,
	}
}

#[derive(Clone, Copy)]
struct AdjacentTileIDs {
	adj: u8,
	i: TurfID,
	max_x: i32,
	max_y: i32,
	count: u8,
}

impl Iterator for AdjacentTileIDs {
	type Item = (u8, TurfID);

	fn next(&mut self) -> Option<Self::Item> {
		loop {
			if self.count > 6 {
				return None;
			}
			self.count += 1;
			let bit = 1 << (self.count - 1);
			if self.adj & bit == bit {
				return Some((
					self.count - 1,
					adjacent_tile_id(self.count - 1, self.i, self.max_x, self.max_y),
				));
			}
		}
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		(0, Some(self.adj.count_ones() as usize))
	}
}

use std::iter::FusedIterator;

impl FusedIterator for AdjacentTileIDs {}

fn adjacent_tile_ids(adj: u8, i: TurfID, max_x: i32, max_y: i32) -> AdjacentTileIDs {
	AdjacentTileIDs {
		adj,
		i,
		max_x,
		max_y,
		count: 0,
	}
}
