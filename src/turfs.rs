pub mod fda;

#[cfg(feature = "monstermos")]
pub mod monstermos;

#[cfg(feature = "putnamos")]
pub mod putnamos;

use super::gas::gas_mixture::GasMixture;

use parking_lot::RwLock;

use std::collections::{BTreeMap, BTreeSet};

use tinyvec::ArrayVec;

use std::sync::atomic::{Ordering,AtomicI8};

use auxtools::*;

use crate::constants::*;

use crate::GasMixtures;

use dashmap::DashMap;

use rayon;

use rayon::prelude::*;

const NORTH: u8 = 1;
const SOUTH: u8 = 2;
const EAST: u8 = 4;
const WEST: u8 = 8;
//const UP: u8 = 16;
//const DOWN: u8 = 32;

const SSAIR_NAME: &str = "SSair";

type TurfID = u32;

// TurfMixture can be treated as "immutable" for all intents and purposes--put other data somewhere else
#[derive(Clone, Copy, Default)]
struct TurfMixture {
	pub mix: usize,
	pub adjacency: u8,
	pub simulation_level: u8,
	pub planetary_atmos: Option<&'static str>,
}

#[allow(dead_code)]
impl TurfMixture {
	pub fn is_immutable(&self) -> bool {
		let mut res = false;
		GasMixtures::with_all_mixtures(|all_mixtures| {
			res = all_mixtures
				.get(self.mix)
				.expect(&format!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.is_immutable()
		});
		res
	}
	pub fn return_pressure(&self) -> f32 {
		let mut res = 0.0;
		GasMixtures::with_all_mixtures(|all_mixtures| {
			res = all_mixtures
				.get(self.mix)
				.expect(&format!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.return_pressure()
		});
		res
	}
	pub fn total_moles(&self) -> f32 {
		let mut res = 0.0;
		GasMixtures::with_all_mixtures(|all_mixtures| {
			res = all_mixtures
				.get(self.mix)
				.expect(&format!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.total_moles()
		});
		res
	}
	pub fn clear_air(&self) {
		GasMixtures::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(self.mix)
				.expect(&format!("Gas mixture not found for turf: {}", self.mix))
				.write()
				.clear();
		});
	}
	pub fn get_gas_copy(&self) -> GasMixture {
		let mut ret: GasMixture = GasMixture::new();
		GasMixtures::with_all_mixtures(|all_mixtures| {
			let to_copy = all_mixtures
				.get(self.mix)
				.expect(&format!("Gas mixture not found for turf: {}", self.mix))
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

#[derive(Default)]
struct TurfZone {
	pub turfs: BTreeMap<TurfID, TurfMixture>,
	pub cooldown: AtomicI8,
	pub next_cooldown: AtomicI8,
}

impl TurfZone {
	pub fn check_cooldown(&self) -> bool {
		if self.cooldown.fetch_sub(1,Ordering::Relaxed) > 1 {
			false
		} else {
			self.cooldown.store(self.next_cooldown.load(Ordering::Relaxed),Ordering::Relaxed);
			self.next_cooldown.store(self.next_cooldown.load(Ordering::Relaxed)+1,Ordering::Relaxed);
			true
		}
	}
	pub fn reset_cooldown(&self) {
		self.next_cooldown.store(1,Ordering::Relaxed);
		self.cooldown.store(0,Ordering::Relaxed);
	}
}

#[derive(Default)]
struct TurfZones {
	pub zones: Vec<RwLock<TurfZone>>,
}

impl TurfZones {
	pub fn get_zone(&self, turf: &TurfID) -> Option<usize> {
		for (i, z) in self.zones.iter().enumerate() {
			if z.read().turfs.contains_key(turf) {
				return Some(i);
			}
		}
		return None;
	}
	fn update_adjacency(&self, i: &TurfID, adj: u8) {
		for z in self.zones.iter() {
			if let Some(t) = z.write().turfs.get_mut(i) {
				t.adjacency = adj;
			}
		}
	}
	pub fn add_zone_from_set(&mut self, new_zone_set: &BTreeSet<TurfID>) {
		self.zones.push(RwLock::new(TurfZone {
			turfs: new_zone_set
				.iter()
				.filter_map(|i| {
					if let Some(t) = TURF_GASES.get(i) {
						Some((*i, *t))
					} else {
						None
					}
				})
				.collect(),
			cooldown: AtomicI8::new(0),
			next_cooldown: AtomicI8::new(1),
		}));
	}
	pub fn set_zones(&mut self, new_zones: &Vec<BTreeSet<TurfID>>) {
		self.zones.clear();
		self.zones.reserve(new_zones.len());
		for set in new_zones {
			self.add_zone_from_set(set);
		}
	}
	pub fn garbage_collect(&mut self) {
		self.zones.retain(|v| !v.read().turfs.is_empty());
	}
	pub fn update_zones(
		&mut self,
		turf: &TurfID,
		orig_adj: u8,
		new_adj: u8,
		max_x: i32,
		max_y: i32,
	) {
		if self.get_zone(turf).is_none() {
			return;
		}
		// first we check if we need to merge any
		if new_adj != 0 {
			let mut networks: BTreeSet<usize> = BTreeSet::new();
			for (_, adj) in adjacent_tile_ids(new_adj, *turf, max_x, max_y) {
				if let Some(zone_id) = self.get_zone(&adj) {
					networks.insert(zone_id);
				}
			}
			if !networks.is_empty() {
				let mut iter = networks.iter();
				let main_zone_id = iter.next().unwrap();
				let mut main_zone = self.zones.get(*main_zone_id).unwrap().write();
				for other_zone_id in iter {
					main_zone
						.turfs
						.append(&mut self.zones.get(*other_zone_id).unwrap().write().turfs);
				}
				main_zone.reset_cooldown();
			}
		} else {
			let mut sets: ArrayVec<[BTreeSet<u32>; 6]> = ArrayVec::new();
			for (_, adj) in adjacent_tile_ids(orig_adj, *turf, max_x, max_y) {
				let mut already_in_set = false;
				for set in sets.iter() {
					already_in_set |= set.contains(&adj);
					if already_in_set {
						break;
					}
				}
				if !already_in_set {
					sets.push(flood_fill_turfs(&adj, max_x, max_y));
				}
			}
			if sets.len() > 1 {
				for set in sets.iter() {
					if let Some(sample) = set.iter().next() {
						self.zones.retain(|v| !v.read().turfs.contains_key(sample));
						self.add_zone_from_set(set);
					}
				}
			}
		}
		self.update_adjacency(&turf,new_adj);
		self.garbage_collect();
	}
}

lazy_static! {
	// All the turfs that have gas mixtures.
	static ref TURF_GASES: DashMap<TurfID, TurfMixture> = DashMap::new();
	// Turfs organized by zone, for faster processing.
	static ref TURF_ZONES: RwLock<TurfZones> = RwLock::new(Default::default());
	// Turf visibility hashes.
	static ref TURF_VIS: DashMap<TurfID, u64> = DashMap::new();
	// Turfs with temperatures/heat capacities. This is distinct from the above.
	static ref TURF_TEMPERATURES: DashMap<TurfID, ThermalInfo> = DashMap::new();
	// We store planetary atmos by hash of the initial atmos string here for speed.
	static ref PLANETARY_ATMOS: DashMap<&'static str, GasMixture> = DashMap::new();
	// For monstermos or other hypothetical fast-process systems.
	static ref HIGH_PRESSURE_TURFS: (
		flume::Sender<TurfID>,
		flume::Receiver<TurfID>
	) = flume::unbounded();
}

fn flood_fill_turfs(starter: &TurfID, max_x: i32, max_y: i32) -> BTreeSet<TurfID> {
	let mut queue: Vec<(TurfID, u8)> = Vec::with_capacity(1024);
	let mut flood: BTreeSet<TurfID> = BTreeSet::new();
	if let Some(start_turf) = TURF_GASES.get(starter) {
		queue.push((*starter, start_turf.adjacency));
	}
	while !queue.is_empty() {
		let (i, adjacency) = queue.pop().unwrap();
		flood.insert(i);
		for (_, adj) in adjacent_tile_ids(adjacency, i, max_x, max_y) {
			if !flood.contains(&adj) {
				if let Some(new_t) = TURF_GASES.get(&adj) {
					queue.push((adj, new_t.adjacency));
				}
			}
		}
	}
	return flood;
}

#[hook("/datum/controller/subsystem/adjacent_air/proc/build_zones")]
fn _hook_build_zones() {
	let max_x = ctx.get_world().get_number("maxx")? as i32;
	let max_y = ctx.get_world().get_number("maxy")? as i32;
	rayon::spawn(move || {
		let mut all_seen: BTreeSet<TurfID> = BTreeSet::new();
		let mut new_zones: Vec<BTreeSet<TurfID>> = Vec::with_capacity(64);
		for r in TURF_GASES.iter() {
			if r.value().simulation_level == SIMULATION_LEVEL_NONE || all_seen.contains(r.key()) {
				continue;
			}
			let mut flood_fill = flood_fill_turfs(r.key(), max_x, max_y);
			new_zones.push(flood_fill.clone());
			all_seen.append(&mut flood_fill);
		}
		TURF_ZONES.write().set_zones(&new_zones);
	});
	Ok(Value::null())
}

#[hook("/turf/proc/update_air_ref")]
fn _hook_register_turf() {
	let simulation_level = args[0].as_number()?;
	if simulation_level < 0.0 {
		TURF_GASES.remove(&unsafe { src.value.data.id });
		Ok(Value::null())
	} else {
		let mut to_insert: TurfMixture = Default::default();
		to_insert.mix = src
			.get("air")?
			.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize;
		to_insert.simulation_level = args[0].as_number()? as u8;
		if let Ok(is_planet) = src.get_number("planetary_atmos") {
			if is_planet != 0.0 {
				if let Ok(at_str) = src.get_string("initial_gas_mix") {
					to_insert.planetary_atmos = Some(Box::leak(at_str.into_boxed_str()));
					let mut entry = PLANETARY_ATMOS
						.entry(to_insert.planetary_atmos.unwrap())
						.or_insert(to_insert.get_gas_copy());
					entry.mark_immutable();
				}
			}
		}
		TURF_GASES.insert(unsafe { src.value.data.id }, to_insert);
		Ok(Value::null())
	}
}

#[hook("/turf/proc/__auxtools_update_turf_temp_info")]
fn _hook_turf_update_temp() {
	if src.get_number("thermal_conductivity").unwrap_or_default() > 0.0
		&& src.get_number("heat_capacity").unwrap_or_default() > 0.0
	{
		let mut entry = TURF_TEMPERATURES
			.entry(unsafe { src.value.data.id })
			.or_insert_with(|| ThermalInfo {
				temperature: 293.15,
				thermal_conductivity: 0.0,
				heat_capacity: 0.0,
				adjacency: NORTH | SOUTH | WEST | EAST,
				adjacent_to_space: false,
			});
		entry.thermal_conductivity = src.get_number("thermal_conductivity")?;
		entry.heat_capacity = src.get_number("heat_capacity")?;
		entry.adjacency = NORTH | SOUTH | WEST | EAST;
		entry.adjacent_to_space = args[0].as_number()? != 0.0;
		entry.temperature = src.get_number("initial_temperature")?;
	} else {
		TURF_TEMPERATURES.remove(&unsafe { src.value.data.id });
	}
	Ok(Value::null())
}

#[hook("/turf/proc/set_sleeping")]
fn _hook_sleep() {
	let arg = if let Some(arg_get) = args.get(0) {
		// null is falsey in byond so
		arg_get.as_number()?
	} else {
		0.0
	};
	if arg == 0.0 {
		TURF_GASES
			.entry(unsafe { src.value.data.id })
			.and_modify(|turf| {
				turf.simulation_level &= !SIMULATION_LEVEL_DISABLED;
			});
	} else {
		TURF_GASES
			.entry(unsafe { src.value.data.id })
			.and_modify(|turf| {
				turf.simulation_level |= SIMULATION_LEVEL_DISABLED;
			});
	}
	Ok(Value::from(true))
}

#[hook("/turf/proc/__update_auxtools_turf_adjacency_info")]
fn _hook_adjacent_turfs() {
	let max_x = ctx.get_world().get_number("maxx")? as i32;
	let max_y = ctx.get_world().get_number("maxy")? as i32;
	let id = unsafe { src.value.data.id };
	if let Ok(adjacent_list) = src.get_list("atmos_adjacent_turfs") {
		let mut adjacency = 0;
		for i in 1..adjacent_list.len() + 1 {
			adjacency |= adjacent_list.get(&adjacent_list.get(i)?)?.as_number()? as i8;
		}
		TURF_GASES.entry(id).and_modify(|turf| {
			let orig = turf.adjacency;
			turf.adjacency = adjacency as u8;
			rayon::spawn(move || {
				TURF_ZONES
					.write()
					.update_zones(&id, orig, adjacency as u8, max_x, max_y);
			});
		});
	} else {
		TURF_GASES.entry(id).and_modify(|turf| {
			let orig = turf.adjacency;
			turf.adjacency = 0;
			rayon::spawn(move || {
				TURF_ZONES.write().update_zones(&id, orig, 0, max_x, max_y);
			});
		});
	}
	if let Ok(atmos_blocked_directions) = src.get_number("conductivity_blocked_directions") {
		let adjacency = NORTH | SOUTH | WEST | EAST & !(atmos_blocked_directions as u8);
		TURF_TEMPERATURES
			.entry(unsafe { src.value.data.id })
			.and_modify(|entry| {
				entry.adjacency = adjacency;
			})
			.or_insert_with(|| ThermalInfo {
				temperature: src.get_number("initial_temperature").unwrap(),
				thermal_conductivity: src.get_number("thermal_conductivity").unwrap(),
				heat_capacity: src.get_number("heat_capacity").unwrap(),
				adjacency: adjacency,
				adjacent_to_space: args[0].as_number().unwrap() != 0.0,
			});
	}
	Ok(Value::null())
}

#[hook("/turf/proc/return_temperature")]
fn _hook_turf_temperature() {
	if let Some(temp_info) = TURF_TEMPERATURES.get(&unsafe { src.value.data.id }) {
		Ok(Value::from(temp_info.temperature))
	} else {
		src.get("initial_temperature")
	}
}

#[hook("/turf/proc/set_temperature")]
fn _hook_set_temperature() {
	let argument = args
		.get(0)
		.ok_or_else(|| runtime!("Invalid argument count to turf temperature set: 0"))?
		.as_number()?;
	TURF_TEMPERATURES
		.entry(unsafe { src.value.data.id })
		.and_modify(|turf| {
			turf.temperature = argument;
		})
		.or_insert_with(|| ThermalInfo {
			temperature: argument,
			thermal_conductivity: src.get_number("thermal_conductivity").unwrap(),
			heat_capacity: src.get_number("heat_capacity").unwrap(),
			adjacency: 0,
			adjacent_to_space: false,
		});
	Ok(Value::null())
}

/*Miserable performance, I mean seriously terrible, 1000x worse than in byond, why?
#[hook("/turf/open/proc/update_visuals")]
fn _hook_update_visuals() {
	use super::gas;
	let old_overlay_types_value = src.get("atmos_overlay_types")?;
	let mut overlay_types: Vec<Value> = Vec::new();
	let gas_overlays = ctx.get_global("meta_gas_overlays")?.as_list()?;
	GasMixtures::with_all_mixtures(|all_mixtures| {
		all_mixtures
			.get(
				TURF_GASES
					.get(&unsafe { src.value.data.id })
					.unwrap()
					.mix,
			)
			.unwrap()
			.read()
			.for_each_gas(|(i, &n)| {
				if let Some(amt) = gas::gas_visibility(i) {
					if n > amt {
						if let Ok(this_gas_overlays_v) =
							gas_overlays.get(gas::gas_id_to_type(i).unwrap().as_ref())
						{
							if let Ok(this_gas_overlays) = this_gas_overlays_v.as_list() {
								let this_idx = FACTOR_GAS_VISIBLE_MAX
									.min((n / MOLES_GAS_VISIBLE_STEP).ceil()) as u32;
								if let Ok(this_gas_overlay) = this_gas_overlays.get(this_idx + 1) {
									overlay_types.push(this_gas_overlay);
								}
							}
						}
					}
				}
			});
	});
	if overlay_types.is_empty() {
		return Ok(Value::null());
	}
	if let Ok(old_overlay_types) = old_overlay_types_value.as_list() {
		if old_overlay_types.len() as usize == overlay_types.len() {
			if overlay_types.iter().enumerate().all(|(i, v)| {
				let v1 = v.value;
				let v2 = old_overlay_types.get((i+1) as u32).unwrap().value;
				unsafe { v1.data.id == v2.data.id && v1.tag == v2.tag }
			}) {
				return Ok(Value::null());
			}
		}
	}
	let l = List::new();
	for v in overlay_types.iter() {
		l.append(v);
	}
	src.call("set_visuals", &[&Value::from(l)])
}
*/

pub const SIMULATION_LEVEL_NONE: u8 = 0;
pub const SIMULATION_LEVEL_DIFFUSE: u8 = 1;
pub const SIMULATION_LEVEL_ALL: u8 = 2;
pub const SIMULATION_LEVEL_DISABLED: u8 = 4;

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
			} else {
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
