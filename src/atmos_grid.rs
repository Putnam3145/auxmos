use super::gas::gas_mixture::GasMixture;

use super::gas::GasMixtures;

use turf_grid::*;

use dm::*;

use super::gas::constants::*;

use std::collections::BTreeMap;

use std::sync::RwLock;

#[derive(Clone, Default)]
struct TurfMixture {
	/// An index into the thread local gases mix.
	pub mix: usize,
	pub adjacency: i8,
	pub simulated: bool,
}

lazy_static! {
	static ref TURF_GASES: RwLock<BTreeMap<usize, TurfMixture>> = RwLock::new(BTreeMap::new());
}

#[hook("/turf/proc/update_air_ref")]
fn _hook_register_turf() {
	let mut to_insert: TurfMixture = Default::default();
	to_insert.mix = src
		.get("air")?
		.get_number("_extools_pointer_gasmixture")?
		.to_bits() as usize;
	to_insert.simulated = args[0].as_number()? > 0.0;
	TURF_GASES
		.write()
		.unwrap()
		.insert(unsafe { src.value.data.id as usize }, to_insert);
	Ok(Value::null())
}

#[hook("/turf/proc/__update_extools_adjacent_turfs")]
fn _hook_adjacent_turfs() {
	if let Ok(adjacent_list) = src.get_list("atmos_adjacent_turfs") {
		let id: usize;
		unsafe {
			id = src.value.data.id as usize;
		}
		if let Some(turf) = TURF_GASES.write().unwrap().get_mut(&id) {
			turf.adjacency = 0;
			for i in 1..adjacent_list.len() + 1 {
				turf.adjacency |= adjacent_list.get(&adjacent_list.get(i)?)?.as_number()? as i8;
			}
		}
	}
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
	let max_x = TurfGrid::max_x() as usize;
	let max_y = TurfGrid::max_y() as usize;
	let mut changed: Vec<(usize, GasMixture)> = Vec::new();
	let mut gases: BTreeMap<usize, (usize, GasMixture, i8)> = BTreeMap::new();
	GasMixtures::with_all_mixtures(|all_mixes| {
		gases = TURF_GASES
			.read()
			.unwrap()
			.iter()
			.filter_map(|(i, m)| {
				if m.simulated {
					Some((*i, (m.mix, all_mixes.get(m.mix).unwrap().clone(), m.adjacency)))
				} else {
					None
				}
			})
			.collect();
	});
	for (i, (mix_num, gas, adj)) in gases.iter() {
		/*
		We're checking an individual tile now. First, we get the adjacency of this tile. This is
		saved by a turf every single time it gets its adjacent turfs updated, and of course this
		processes everything in one go, blocking the byond thread until it's done (it's called from a hook),
		so it'll be nice and consistent.
		*/
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
			end_gas.merge(&(gases.get(&(i + max_x)).unwrap().1.clone() * coeff));
		}
		// SOUTH
		if adj & 2 == 2 {
			end_gas.merge(&(gases.get(&(i - max_x)).unwrap().1.clone() * coeff));
		}
		// EAST
		if adj & 4 == 4 {
			end_gas.merge(&(gases.get(&(i + 1)).unwrap().1.clone() * coeff));
		}
		// WEST
		if adj & 8 == 8 {
			end_gas.merge(&(gases.get(&(i - 1)).unwrap().1.clone() * coeff));
		}
		// UP (I actually don't know if byond is +Z up or down, but Z up is standard, I'm reasonably sure, so.)
		if adj & 16 == 16 {
			end_gas.merge(&(gases.get(&(i + (max_y * max_x))).unwrap().1.clone() * coeff));
		}
		// DOWN
		if adj & 32 == 32 {
			end_gas.merge(&(gases.get(&(i - (max_y * max_x))).unwrap().1.clone() * coeff));
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
		changed.push((
			*mix_num,
			gas.clone() + &(end_gas + &(gas * (-(adj_amount as f32) * coeff))),
		));
	}
	GasMixtures::copy_from_mixtures(&changed);
	Ok(Value::null())
}
