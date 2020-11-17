pub mod fda;

#[cfg(feature = "monstermos")]
pub mod monstermos;

use super::gas::gas_mixture::GasMixture;

use turf_grid::*;

use dm::*;

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
				.unwrap()
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
				.unwrap()
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
				.unwrap()
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
				.unwrap()
				.clear();
		});
	}
	pub fn get_gas_copy(&self) -> GasMixture {
		let mut ret: GasMixture = GasMixture::new();
		GasMixtures::with_all_mixtures(|all_mixtures| {
			let to_copy = all_mixtures
				.get(self.mix)
				.expect(&format!("Gas mixture not found for turf: {}", self.mix))
				.read()
				.unwrap();
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

lazy_static! {
	static ref TURF_GASES: DashMap<usize, TurfMixture> = DashMap::new();
	static ref TURF_TEMPERATURES: DashMap<usize, ThermalInfo> = DashMap::new();
	static ref PLANETARY_ATMOS: DashMap<&'static str, GasMixture> = DashMap::new();
}

#[hook("/turf/proc/update_air_ref")]
fn _hook_register_turf() {
	let mut to_insert: TurfMixture = Default::default();
	to_insert.mix = src
		.get("air")?
		.get_number("_extools_pointer_gasmixture")?
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
	TURF_GASES.insert(unsafe { src.value.data.id as usize }, to_insert);
	Ok(Value::null())
}

#[hook("/turf/proc/__auxtools_update_turf_temp_info")]
fn _hook_turf_update_temp() {
	let mut entry = TURF_TEMPERATURES
		.entry(unsafe { src.value.data.id as usize })
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
	Ok(Value::null())
}

#[hook("/turf/proc/__update_auxtools_turf_adjacency_info")]
fn _hook_adjacent_turfs() {
	if let Ok(adjacent_list) = src.get_list("atmos_adjacent_turfs") {
		let id: usize;
		unsafe {
			id = src.value.data.id as usize;
		}
		let mut adjacency = 0;
		for i in 1..adjacent_list.len() + 1 {
			adjacency |= adjacent_list.get(&adjacent_list.get(i)?)?.as_number()? as i8;
		}
		if let Some(mut turf) = TURF_GASES.get_mut(&id) {
			turf.adjacency = adjacency as u8;
		}
	}
	if let Ok(atmos_supeconductivity) = src.get_number("conductivity_blocked_directions") {
		let adjacency = NORTH | SOUTH | WEST | EAST & !(atmos_supeconductivity as u8);
		let mut entry = TURF_TEMPERATURES
			.entry(unsafe { src.value.data.id as usize })
			.or_insert_with(|| ThermalInfo {
				temperature: src.get_number("temperature").unwrap(),
				thermal_conductivity: src.get_number("thermal_conductivity").unwrap(),
				heat_capacity: src.get_number("heat_capacity").unwrap(),
				adjacency: adjacency,
				adjacent_to_space: args[0].as_number().unwrap() != 0.0,
			});
		entry.adjacency = adjacency;
	}
	Ok(Value::null())
}
#[cfg(feature = "monstermos")]
const SIMULATION_LEVEL_NONE: u8 = 0;
//const SIMULATION_LEVEL_SHARE_FROM: u8 = 1;
const SIMULATION_LEVEL_SIMULATE: u8 = 2;

fn adjacent_tile_id(id: u8, i: usize, max_x: i32, max_y: i32) -> usize {
	let z_size = max_x * max_y;
	let i = i as i32;
	match id {
		0 => (i + max_x) as usize,
		1 => (i - max_x) as usize,
		2 => (i + 1) as usize,
		3 => (i - 1) as usize,
		4 => (i + z_size) as usize,
		5 => (i - z_size) as usize,
		_ => i as usize,
	}
}

fn adjacent_tile_ids(adj: u8, i: usize, max_x: i32, max_y: i32) -> Vec<usize> {
	let mut ret = Vec::with_capacity(adj.count_ones() as usize);
	for j in 0..6 {
		let bit = 1 << j;
		if adj & bit == bit {
			ret.push(adjacent_tile_id(j, i, max_x, max_y));
		}
	}
	ret
}
