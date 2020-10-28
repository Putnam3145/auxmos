use super::gas::gas_mixture::GasMixture;

use turf_grid::*;

use dm::*;

use super::gas::constants::*;

use std::collections::BTreeMap;

use std::sync::RwLock;

#[derive(Clone, Default)]
pub struct TurfMixture {
	pub mix: super::gas::gas_mixture::GasMixture,
	pub adjacency: i8,
}

lazy_static! {
	static ref TURF_GASES: RwLock<BTreeMap<u32, TurfMixture>> = RwLock::new(BTreeMap::new());
}

pub fn turf_gases() -> &'static RwLock<BTreeMap<u32, TurfMixture>> {
	&TURF_GASES
}

#[hook("/turf/proc/__update_extools_adjacent_turfs")]
fn _hook_adjacent_turfs() {
	let adjacent_list = src.get_list("atmos_adjacent_turfs")?;
	let id: u32;
	unsafe {
		id = src.value.data.id;
	}
	if let Some(turf) = TURF_GASES.write().unwrap().get_mut(&id) {
		turf.adjacency = 0;
		for i in 0..adjacent_list.len() {
			turf.adjacency |= adjacent_list.get(i)?.as_number()? as i8;
		}
		Ok(Value::null())
	} else {
		Err(runtime!(
			"Turf {} tried to update its adjacent turfs with no air!",
			src
		))
	}
}

#[hook("/datum/gas_mixture/turf/__gasmixture_register")]
fn _hook_turf_mix_register(turf: Value) {
	let id: u32;
	unsafe {
		id = turf.value.data.id;
	}
	TURF_GASES.write().unwrap().insert(id, Default::default());
	src.set("_extools_pointer_gasmixture", -(id as f32));
	Ok(Value::null())
}
#[hook("/datum/gas_mixture/turf/__gasmixture_unregister")]
fn _hook_turf_mix_unregister() {
	TURF_GASES
		.write()
		.unwrap()
		.remove(&(-src.get_number("_extools_pointer_gasmixture")? as u32));
	src.set("_extools_pointer_gasmixture", &Value::null());
	Ok(Value::null())
}

#[hook("/datum/controller/subsystem/air/proc/process_turfs_extools")]
fn _process_turf_hook() {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature (sleeping turfs).
		It can run in parallel, but doesn't yet. We'll see if it's required for performance reasons.
	*/
	// First we copy the gas list immutably, so we can be sure this is consistent.
	let max_x = TurfGrid::max_x() as u32;
	let max_y = TurfGrid::max_y() as u32;
	let archive = TURF_GASES.read().unwrap().clone();
	let mut gas = TURF_GASES.write().unwrap();
	let iter = gas
		.iter_mut()
		.zip(archive.iter())
		.filter_map(|((i, cur_gas), (_, cur_archive))| {
			/*
			We're checking an individual tile now. First, we get the adjacency of this tile. This is
			saved by a turf every single time it gets its adjacent turfs updated, and of course this
			processes everything in one go, blocking the byond thread until it's done (it's called from a hook),
			so it'll be nice and consistent.
			*/
			let adj = cur_gas.adjacency;
			let adj_amount = adj.count_ones();
			/*
			If we don't multiply by this coefficient, the system is unstable, and will
			rapidly diverge and generate infinite or negative molages.
			In short: bad juju.
			*/
			let coeff = 1.0 / (adj_amount as f32 + 1.0);
			// We build the gas from each individual adjacent turf, starting from nothing.
			let mut end_gas: GasMixture = GasMixture::from_vol(CELL_VOLUME);
			// NORTH (Byond is +y-up)
			if adj & 1 == 1 {
				end_gas.merge(
					&(archive
						.get(&(i + max_x))
						.unwrap_or(&Default::default())
						.mix
						.clone() * coeff),
				);
			}
			// SOUTH
			if adj & 2 == 2 {
				end_gas.merge(
					&(archive
						.get(&(i - max_x))
						.unwrap_or(&Default::default())
						.mix
						.clone() * coeff),
				);
			}
			// EAST
			if adj & 4 == 4 {
				end_gas.merge(
					&(archive
						.get(&(i + 1))
						.unwrap_or(&Default::default())
						.mix
						.clone() * coeff),
				);
			}
			// WEST
			if adj & 8 == 8 {
				end_gas.merge(
					&(archive
						.get(&(i - 1))
						.unwrap_or(&Default::default())
						.mix
						.clone() * coeff),
				);
			}
			// UP (I actually don't know if byond is +Z up or down, but Z up is standard, I'm reasonably sure, so.)
			if adj & 16 == 16 {
				end_gas.merge(
					&(archive
						.get(&(i + (max_y * max_x)))
						.unwrap_or(&Default::default())
						.mix
						.clone() * coeff),
				);
			}
			// DOWN
			if adj & 32 == 32 {
				end_gas.merge(
					&(archive
						.get(&(i - (max_y * max_x)))
						.unwrap_or(&Default::default())
						.mix
						.clone() * coeff),
				);
			}
			/*
			This is the weird bit, of course.
			We merge our gas with the combined gases of the others... plus
			our own archive, multiplied by the coefficient multiplied
			by the amount of adjacent turfs times negative 1. This is the step
			that simulates "sharing"; the negative-moled gas mix returned by the right hand side
			of the end_gas + [stuff] expression below represents "gas removal" more than an
			actual gas mix. A virtual gas mix, so to speak.

			Either way: the result is that the gas mix is set to what it would be if it
			equally shared itself with the other tiles, plus kept part of itself in.

			Come to think, it may be fully possible that this is exactly equal to just
			adding all of the gas mixes together, then multiplying it by the total amount of mixes.

			Someone should do the math on that.
			*/
			cur_gas
				.mix
				.merge(&(end_gas + &(cur_archive.mix.clone() * (-(adj_amount as f32) * coeff))));
			if cur_gas.mix.compare(&cur_archive.mix) != -2 || cur_gas.mix.can_react() {
				Some(i)
			} else {
				None
			}
		});
	let ret_list = List::with_size(iter.size_hint().0 as u32);
	for &id in iter {
		ret_list.append(&TurfGrid::turf_by_id(id))
	}
	Ok(Value::from(ret_list))
}
