pub mod fda;

#[cfg(feature = "monstermos")]
pub mod monstermos;

#[cfg(feature = "putnamos")]
pub mod putnamos;

use super::gas::gas_mixture::GasMixture;

use auxtools::*;

use std::sync::atomic::{AtomicI32, AtomicU64, AtomicU8, Ordering};

use parking_lot::RwLock;

use itertools::Itertools;

use std::cell::Cell;

use crate::constants::*;

use crate::GasMixtures;

use dashmap::DashMap;

use rayon;

use rayon::prelude::*;

const NORTH: u8 = 1;
const SOUTH: u8 = 2;
const EAST: u8 = 4;
const WEST: u8 = 8;
const UP: u8 = 16;
const DOWN: u8 = 32;

type TurfID = u32;

static MAX_X: AtomicI32 = AtomicI32::new(255);
static TURFS_PER_Z: AtomicI32 = AtomicI32::new(65025);
static MAX_Z: AtomicU8 = AtomicU8::new(0);

// TurfMixture can be treated as "immutable" for all intents and purposes--put other data somewhere else
#[derive(Clone, Copy, Default)]
struct TurfMixture {
	pub mix: usize,
	pub adjacency: u8,
	pub simulation_level: u8,
	pub planetary_atmos: Option<u64>,
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
#[derive(Clone)]
struct ThermalInfo {
	_temperature: Cell<f32>,
	pub thermal_conductivity: f32,
	pub heat_capacity: f32,
	pub adjacency: u8,
	pub adjacent_to_space: bool,
}

impl ThermalInfo {
	unsafe fn set_temperature(&self, new_temp: f32) {
		self._temperature.set(new_temp);
	}
	fn get_temperature(&self) -> f32 {
		self._temperature.get()
	}
}

/*
	Again with the sync cell, but: there are precisely two places in the entire codebase
	where temperature is set: during heat processing and during turf initialization.
	We can thus expect no race conditions. I think. I'll make the setter unsafe.
*/
unsafe impl Sync for ThermalInfo {}

unsafe impl Send for ThermalInfo {}

#[derive(Clone)]
struct Turf<V: Clone> {
	_id: TurfID,
	value: V,
}

impl<V: Copy> Copy for Turf<V> {}

impl<V: Clone> Turf<V> {
	pub fn id(&self) -> TurfID {
		self._id
	}
	pub fn new(_id: TurfID, value: V) -> Self {
		Turf { _id, value }
	}
}

use std::ops::{Deref, DerefMut};

impl<V: Clone> Deref for Turf<V> {
	type Target = V;

	fn deref(&self) -> &V {
		&self.value
	}
}

impl<V: Clone> DerefMut for Turf<V> {
	fn deref_mut(&mut self) -> &mut V {
		&mut self.value
	}
}

type DeferredOp<T> = Box<dyn Fn(&mut T) + Send + Sync + 'static>;

struct TurfMap<V: Clone> {
	z_levels: Vec<RwLock<Vec<Turf<V>>>>,
	z_size: usize,
	max_x: i32,
	max_y: i32,
	write_operations: crossbeam::queue::ArrayQueue<(TurfID,DeferredOp<Turf<V>>)>,
}

#[allow(dead_code)]
impl<V: Clone> TurfMap<V> {
	// type Container = RwLock<Vec<(TurfID,V)>>
	fn new() -> Self {
		TurfMap {
			z_levels: Vec::with_capacity(16),
			z_size: 255 * 255,
			max_x: 255,
			max_y: 255,
			write_operations: crossbeam::queue::ArrayQueue::new(1024),
		}
	}
	pub fn x(&self, id: TurfID) -> usize {
		(id as i32 % self.max_x) as usize
	}
	pub fn y(&self, id: TurfID) -> usize {
		((id as i32 % (self.max_x * self.max_y)) / self.max_x) as usize
	}
	pub fn z(&self, id: TurfID) -> usize {
		(id) as usize / self.z_size
	}
	pub fn has_z(&self, id: TurfID) -> bool {
		self.z_levels.len() > self.z(id)
	}
	fn do_deferred_funcs() {

	}
	pub(crate) fn with_z<T>(&self, z: usize, f: impl FnOnce(&RwLock<Vec<Turf<V>>>) -> T) -> T {
		f(self.z_levels.get(z).unwrap())
	}
	pub(crate) fn with_key<T>(
		&self,
		id: &TurfID,
		f: impl FnOnce(&RwLock<Vec<Turf<V>>>, Result<usize, usize>) -> T,
	) -> T {
		self.with_z(
			self.z(*id),
			|specific_z| {
				let idx = { specific_z.read().binary_search_by_key(id, |turf| turf.id()) };
				f(&specific_z, idx)
			},
		)
	}
	pub fn read<T>(&self, id: &TurfID, f: impl FnOnce(&Turf<V>) -> T) -> Option<T> {
		self.with_key(&id, |z, res| {
			if let Ok(idx) = res {
				Some(f(unsafe { z.read().get_unchecked(idx) }))
			} else {
				None
			}
		})
	}
	pub fn get_copy(&self, id: &TurfID) -> Option<Turf<V>> {
		self.with_key(&id, |z, res| {
			if let Ok(idx) = res {
				Some(unsafe { z.read().get_unchecked(idx).clone() })
			} else {
				None
			}
		})
	}
	unsafe fn do_modification(&self, z: &RwLock<Vec<Turf<V>>>, id: TurfID, idx: usize, f: (impl Fn(&mut Turf<V>) + Send + Sync + 'static)) {
		if let Some(mut lock) = z.try_write() {
			f(&mut lock.get_unchecked_mut(idx));
		} else {
			let _ = self.write_operations.push((id,Box::new(f)));
		}
	}
	pub fn modify(&self, id: TurfID, f: (impl Fn(&mut Turf<V>) + Send + Sync + 'static)) {
		self.with_key(&id, |z, res| {
			if let Ok(idx) = res {
				f(unsafe { &mut z.write().get_unchecked_mut(idx) });
			}
		});
	}
	pub fn modify_or_insert_with(
		&self,
		id: TurfID,
		f: (impl Fn(&mut Turf<V>) + Send + Sync + 'static),
		g: impl Fn() -> V,
	) {
		self.with_key(&id, |z, res| match res {
			Ok(idx) => {
				f(unsafe { &mut z.write().get_unchecked_mut(idx) });
			}
			Err(idx) => {
				z.write().insert(idx, Turf::new(id, g()));
			}
		})
	}
	pub fn remove(&self, id: &TurfID) -> Option<Turf<V>> {
		self.with_key(&id, |z, res| {
			if let Ok(idx) = res {
				Some(z.write().remove(idx))
			} else {
				None
			}
		})
	}
	pub fn insert(&self, id: TurfID, element: V) -> Option<Turf<V>> {
		self.with_key(&id, |z, res| match res {
			Ok(idx) => {
				let mut lock = z.write();
				let reference = unsafe { lock.get_unchecked_mut(idx) };
				let ret = reference.clone();
				**reference = element;
				Some(ret)
			}
			Err(idx) => {
				z.write().insert(idx, Turf::new(id, element));
				None
			}
		})
	}
	pub fn insert_with<F>(&self, id: TurfID, f: F) -> Option<Turf<V>>
	where
		F: FnOnce() -> V,
	{
		self.insert(id, f())
	}
	pub fn resize(&mut self, max_x: i32, max_y: i32, new_z_amount: usize) {
		let z_size = (max_x * max_y) as usize;
		if z_size != self.z_size {
			let all_turfs: Vec<_> = self
				.z_levels
				.iter()
				.flat_map(|l| l.read().iter().cloned().collect::<Vec<_>>())
				.collect();
			let mut new_vecs: Vec<Vec<Turf<V>>> = Vec::with_capacity(new_z_amount);
			for (_, group) in &all_turfs
				.into_iter()
				.group_by(|t| (t.id() / z_size as u32) & 1 == 0)
			{
				new_vecs.push(group.collect::<Vec<_>>());
			}
			for (idx, v) in new_vecs.iter().enumerate() {
				*self.z_levels[idx].write() = v.clone();
			}
		}
		self.z_levels
			.resize_with(new_z_amount, || RwLock::new(Vec::with_capacity(16384)));
	}
	pub fn clear(&mut self) {
		self.z_levels.clear();
	}
	pub fn iter(&self) -> std::slice::Iter<RwLock<Vec<Turf<V>>>> {
		self.z_levels.iter()
	}
	pub fn for_each_adjacent(
		&self,
		adj: u8,
		maybe_z_idx: Option<usize>,
		i: TurfID,
		mut f: impl FnMut(usize, &Turf<V>),
	) {
		let z_idx = maybe_z_idx.unwrap_or_else(|| self.z(i));
		if adj & (NORTH | WEST | EAST | SOUTH) > 0 {
			self.with_z(self.z(i), |lock| {
				let z = lock.read();
				if adj & WEST == WEST {
					f(3, z.get(z_idx - 1).unwrap());
				}
				if adj & EAST == EAST {
					f(2, z.get(z_idx + 1).unwrap());
				}
				if adj & NORTH == NORTH {
					if let Ok(idx) = z.binary_search_by_key(
						&adjacent_tile_id(0, i, self.max_x, self.max_y),
						|e| e.id(),
					) {
						f(0, z.get(idx).unwrap())
					}
				}
				if adj & SOUTH == SOUTH {
					if let Ok(idx) = z.binary_search_by_key(
						&adjacent_tile_id(1, i, self.max_x, self.max_y),
						|e| e.id(),
					) {
						f(1, z.get(idx).unwrap())
					}
				}
			})
		}
		if adj & UP == UP {
			self.read(&adjacent_tile_id(4, i, self.max_x, self.max_y), |turf| {
				f(4, turf)
			});
		}
		if adj & DOWN == DOWN {
			self.read(&adjacent_tile_id(5, i, self.max_x, self.max_y), |turf| {
				f(5, turf)
			});
		}
	}
}

lazy_static! {
	// All the turfs that have gas mixtures.
	static ref TURF_GASES: RwLock<TurfMap<TurfMixture>> = RwLock::new(TurfMap::new());
	// Turfs with temperatures/heat capacities. This is distinct from the above.
	static ref TURF_TEMPERATURES: RwLock<TurfMap<ThermalInfo>> = RwLock::new(TurfMap::new());
	// We store planetary atmos by hash of the initial atmos string here for speed.
	static ref PLANETARY_ATMOS: DashMap<u64, GasMixture> = DashMap::new();
	// these are pretty sparse and totally independent, so, dashmap
	static ref TURF_VIS_HASH: DashMap<TurfID,AtomicU64> = DashMap::new();
	// For monstermos or other hypothetical fast-process systems.
	static ref HIGH_PRESSURE_TURFS: (
		flume::Sender<Vec<TurfID>>,
		flume::Receiver<Vec<TurfID>>
	) = flume::unbounded();
}

fn check_and_update_vis_hash(id: TurfID, hash: u64) -> bool {
	if let Some(existing_hash) = TURF_VIS_HASH.get(&id) {
		existing_hash.swap(hash, Ordering::Relaxed) != hash
	} else {
		TURF_VIS_HASH.insert(id, AtomicU64::new(hash));
		true
	}
}

#[shutdown]
fn _shutdown_turfs() {
	TURF_GASES.write().clear();
	TURF_TEMPERATURES.write().clear();
	PLANETARY_ATMOS.clear();
	TURF_VIS_HASH.clear();
}

#[hook("/world/proc/refresh_atmos_grid")]
fn _hook_refresh_grid() {
	let max_x = auxtools::Value::world().get_number(byond_string!("maxx"))? as i32;
	let max_y = auxtools::Value::world().get_number(byond_string!("maxy"))? as i32;
	let max_z = auxtools::Value::world().get_number(byond_string!("maxz"))? as u8;
	MAX_X.store(max_x, Ordering::Relaxed);
	TURFS_PER_Z.store(max_x * max_y, Ordering::Relaxed);
	MAX_Z.store(max_z, Ordering::Relaxed);
	TURF_GASES.write().resize(max_x, max_y, max_z as usize);
	TURF_TEMPERATURES
		.write()
		.resize(max_x, max_y, max_z as usize);
	Ok(Value::null())
}

#[hook("/turf/proc/update_air_ref")]
fn _hook_register_turf() {
	let simulation_level = args[0].as_number()?;
	if !TURF_GASES.read().has_z(unsafe { src.raw.data.id }) {
		Proc::find("/world/proc/refresh_atmos_grid")
			.unwrap()
			.call(&[])?;
	}
	if simulation_level < 0.0 {
		TURF_GASES.read().remove(&unsafe { src.raw.data.id });
		Ok(Value::null())
	} else {
		let mut to_insert: TurfMixture = Default::default();
		to_insert.mix = src
			.get(byond_string!("air"))?
			.get_number(byond_string!("_extools_pointer_gasmixture"))?
			.to_bits() as usize;
		to_insert.simulation_level = args[0].as_number()? as u8;
		if let Ok(is_planet) = src.get_number(byond_string!("planetary_atmos")) {
			if is_planet != 0.0 {
				use std::hash::Hasher;
				let mut hasher = std::collections::hash_map::DefaultHasher::new();
				if let Ok(at_str) = src.get_string(byond_string!("initial_gas_mix")) {
					hasher.write(at_str.as_bytes());
					let hash = hasher.finish();
					to_insert.planetary_atmos = Some(hash);
					if !PLANETARY_ATMOS.contains_key(&hash) {
						to_insert.planetary_atmos = Some(hash);
						let mut entry = PLANETARY_ATMOS
							.entry(hash)
							.or_insert_with(|| to_insert.get_gas_copy());
						entry.mark_immutable();
					}
				}
			}
		}
		TURF_GASES
			.read()
			.insert(unsafe { src.raw.data.id }, to_insert);
		Ok(Value::null())
	}
}

#[hook("/turf/proc/__auxtools_update_turf_temp_info")]
fn _hook_turf_update_temp() {
	if src
		.get_number(byond_string!("thermal_conductivity"))
		.unwrap_or_default()
		> 0.0 && src
		.get_number(byond_string!("heat_capacity"))
		.unwrap_or_default()
		> 0.0
	{
		TURF_TEMPERATURES.read().insert(
			unsafe { src.raw.data.id },
			ThermalInfo {
				_temperature: Cell::new(src.get_number(byond_string!("initial_temperature"))?),
				thermal_conductivity: src.get_number(byond_string!("thermal_conductivity"))?,
				heat_capacity: src.get_number(byond_string!("heat_capacity"))?,
				adjacency: NORTH | SOUTH | WEST | EAST,
				adjacent_to_space: args[0].as_number()? != 0.0,
			},
		);
	} else {
		TURF_TEMPERATURES.read().remove(&unsafe { src.raw.data.id });
	}
	Ok(Value::null())
}

#[hook("/turf/proc/set_sleeping")]
fn _hook_sleep() {
	let arg = if let Some(arg_get) = args.get(0) {
		// null is falsey in byond so
		arg_get.as_number().unwrap_or_else(|_| 1.0) // and pretty much everything else is truthy
	} else {
		0.0
	};
	if arg == 0.0 {
		TURF_GASES.read().modify(unsafe { src.raw.data.id }, |m| {
			m.simulation_level &= !SIMULATION_LEVEL_DISABLED
		});
	} else {
		TURF_GASES.read().modify(unsafe { src.raw.data.id }, |m| {
			m.simulation_level |= SIMULATION_LEVEL_DISABLED
		});
	}
	Ok(Value::from(true))
}

#[hook("/turf/proc/__update_auxtools_turf_adjacency_info")]
fn _hook_adjacent_turfs() {
	if let Ok(adjacent_list) = src.get_list(byond_string!("atmos_adjacent_turfs")) {
		let mut adjacency = 0;
		for i in 1..adjacent_list.len() + 1 {
			adjacency |= adjacent_list.get(&adjacent_list.get(i)?)?.as_number()? as i8;
		}
		TURF_GASES.read().modify(unsafe { src.raw.data.id }, move |m| {
			m.adjacency = adjacency as u8
		});
	} else {
		TURF_GASES
			.read()
			.modify(unsafe { src.raw.data.id }, |m| m.adjacency = 0);
	}
	if let Ok(atmos_blocked_directions) =
		src.get_number(byond_string!("conductivity_blocked_directions"))
	{
		let adjacency = NORTH | SOUTH | WEST | EAST & !(atmos_blocked_directions as u8);
		TURF_TEMPERATURES.read().modify_or_insert_with(
			unsafe { src.raw.data.id },
			move |entry| {
				entry.adjacency = adjacency;
			},
			|| ThermalInfo {
				_temperature: Cell::new(
					src.get_number(byond_string!("initial_temperature"))
						.unwrap(),
				),
				thermal_conductivity: src
					.get_number(byond_string!("thermal_conductivity"))
					.unwrap(),
				heat_capacity: src.get_number(byond_string!("heat_capacity")).unwrap(),
				adjacency: adjacency,
				adjacent_to_space: args[0].as_number().unwrap() != 0.0,
			},
		);
	}
	Ok(Value::null())
}

#[hook("/turf/proc/return_temperature")]
fn _hook_turf_temperature() {
	TURF_TEMPERATURES
		.read()
		.read(&unsafe { src.raw.data.id }, |temp_info| {
			let t = temp_info.get_temperature();
			if t.is_normal() {
				Ok(Value::from(t))
			} else {
				src.get(byond_string!("initial_temperature"))
			}
		})
		.unwrap_or_else(|| src.get(byond_string!("initial_temperature")))
}

#[hook("/turf/proc/set_temperature")]
fn _hook_set_temperature() {
	let argument = args
		.get(0)
		.ok_or_else(|| runtime!("Invalid argument count to turf temperature set: 0"))?
		.as_number()?;
	TURF_TEMPERATURES.read().modify_or_insert_with(
		unsafe { src.raw.data.id },
		move |turf| unsafe {
			turf.set_temperature(argument);
		},
		|| ThermalInfo {
			_temperature: Cell::new(argument),
			thermal_conductivity: src
				.get_number(byond_string!("thermal_conductivity"))
				.unwrap(),
			heat_capacity: src.get_number(byond_string!("heat_capacity")).unwrap(),
			adjacency: 0,
			adjacent_to_space: false,
		},
	);
	Ok(Value::null())
}

/*Miserable performance, I mean seriously terrible, 1000x worse than in byond, why?
#[hook("/turf/open/proc/update_visuals")]
fn _hook_update_visuals() {
	use super::gas;
	let old_overlay_types_value = src.get(byond_string!("atmos_overlay_types"))?;
	let mut overlay_types: Vec<Value> = Vec::new();
	let gas_overlays = src.get_global("meta_gas_overlays")?.as_list()?;
	GasMixtures::with_all_mixtures(|all_mixtures| {
		all_mixtures
			.get(
				TURF_GASES
					.get(&unsafe { src.raw.data.id })
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
				let v1 = v.raw;
				let v2 = old_overlay_types.get((i+1) as u32).unwrap().raw;
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
pub const SIMULATION_LEVEL_ANY: u8 = SIMULATION_LEVEL_DIFFUSE | SIMULATION_LEVEL_ALL;
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
