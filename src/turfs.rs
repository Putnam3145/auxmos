pub mod fea;

#[cfg(monstermos)]
pub mod monstermos;

use super::gas::gas_mixture::GasMixture;

use super::gas::GasMixtures;

use turf_grid::*;

use dm::*;

use super::gas::constants::*;

use dashmap::DashMap;

use coarsetime::{Duration, Instant};

use flume;

use rayon;

use rayon::prelude::*;

use std::sync::atomic::{AtomicU8, Ordering};

// TODO: tuple type the identifiers, starting in turf_grid over yonder

// TODO: figure out why active turf processing takes over a minute (??)

#[derive(Clone, Copy, Default)]
struct TurfMixture {
	/// An index into the thread local gases mix.
	pub mix: usize,
	pub adjacency: i8,
	pub simulation_level: u8,
	pub planetary_atmos: Option<&'static str>,
}

#[allow(dead_code)]
impl TurfMixture {
	pub fn is_immutable(&self) -> bool {
		let mut ret = false;
		GasMixtures::with_all_mixtures(|all_mixtures| {
			ret = all_mixtures.get(self.mix).unwrap().is_immutable();
		});
		ret
	}
	pub fn total_moles(&self) -> f32 {
		let mut ret = 0.0;
		GasMixtures::with_all_mixtures(|all_mixtures| {
			ret = all_mixtures.get(self.mix).unwrap().total_moles();
		});
		ret
	}
	pub fn clear_air(&self) {
		GasMixtures::with_all_mixtures_mut(|all_mixtures| {
			all_mixtures.get_mut(self.mix).unwrap().clear();
		});
	}
	pub fn get_gas_copy(&self) -> GasMixture {
		let mut mix: GasMixture = GasMixture::new();
		GasMixtures::with_all_mixtures(|all_mixtures| {
			mix = all_mixtures.get(self.mix).unwrap().clone();
		});
		mix
	}
}

lazy_static! {
	static ref TURF_GASES: DashMap<usize, TurfMixture> = DashMap::new();
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
				GasMixtures::with_all_mixtures(|all_mixtures| {
					let mut entry = PLANETARY_ATMOS
						.entry(to_insert.planetary_atmos.unwrap())
						.or_insert(all_mixtures.get(to_insert.mix).unwrap().clone());
					entry.mark_immutable();
				})
			}
		}
	}
	TURF_GASES.insert(unsafe { src.value.data.id as usize }, to_insert);
	Ok(Value::null())
}

#[hook("/turf/proc/__update_extools_adjacent_turfs")]
fn _hook_adjacent_turfs() {
	if let Ok(adjacent_list) = src.get_list("atmos_adjacent_turfs") {
		let id: usize;
		unsafe {
			id = src.value.data.id as usize;
		}
		if let Some(mut turf) = TURF_GASES.get_mut(&id) {
			turf.adjacency = 0;
			for i in 1..adjacent_list.len() + 1 {
				turf.adjacency |= adjacent_list.get(&adjacent_list.get(i)?)?.as_number()? as i8;
			}
		}
	}
	Ok(Value::null())
}
#[cfg(monstermos)]
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

fn adjacent_tile_ids(adj: i8, i: usize, max_x: i32, max_y: i32) -> Vec<usize> {
	let mut ret = Vec::with_capacity(adj.count_ones() as usize);
	for j in 0..6 {
		let bit = 1 << j;
		if adj & bit == bit {
			ret.push(adjacent_tile_id(j, i, max_x, max_y));
		}
	}
	ret
}
