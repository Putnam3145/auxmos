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
	pub simulation_level: u8,
	pub cooldown: i8,
	pub next_cooldown: i8,
}

lazy_static! {
	static ref TURF_GASES: RwLock<BTreeMap<usize, TurfMixture>> = RwLock::new(BTreeMap::new());
}

#[hook("/datum/controller/subsystem/air/proc/add_to_active")]
fn _add_to_active_hook() {
	// yeah, this is just super low priority, first line fails if not open, second line if lock is held (possible if cooldown is happening),
	// third can fail if the turf isn't in the list (which would be strange but, eh, this should never panic or runtime)
	if args[0].get("air").is_ok() {
		if let Ok(mut turfs) = TURF_GASES.try_write() {
			if let Some(turf) = turfs.get_mut(unsafe { &(args[0].value.data.id as usize) }) {
				turf.cooldown = 0;
				turf.next_cooldown = 1;
			}
		}
	}
	Ok(Value::null())
}

#[hook("/turf/proc/update_air_ref")]
fn _hook_register_turf() {
	let mut to_insert: TurfMixture = Default::default();
	to_insert.mix = src
		.get("air")?
		.get_number("_extools_pointer_gasmixture")?
		.to_bits() as usize;
	to_insert.simulation_level = args[0].as_number()? as u8;
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

//const SIMULATION_LEVEL_NONE: u8 = 0;
//const SIMULATION_LEVEL_SHARE_FROM: u8 = 1;
const SIMULATION_LEVEL_SIMULATE: u8 = 2;

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
	use rayon;
	use std::sync::mpsc;
	use std::thread;
	let (sender, receiver) = mpsc::channel();
	let thread = thread::spawn(move || {
		let gases = TURF_GASES.read().unwrap();
		for (i, m) in gases
			.iter()
			.filter(|(_, m)| m.simulation_level >= SIMULATION_LEVEL_SIMULATE && m.cooldown <= 0)
		{
			/*
			We're checking an individual tile now. First, we get the adjacency of this tile. This is
			saved by a turf every single time it gets its adjacent turfs updated, and of course this
			processes everything in one go, blocking the byond thread until it's done (it's called from a hook),
			so it'll be nice and consistent.
			*/
			let adj = m.adjacency;
			let adj_amount = adj.count_ones();
			/*
			We build the gas from each individual adjacent turf, starting from our own
			multiplied by some magic constants relating to diffusion.
			Okay it's not that magic. I'll explain it.
			Let's use the example GAS_DIFFUSION_CONSTANT of 8, and an example
			adjacent turfs count of 3. Each adjacent turf will give this
			turf 1/8 of their own gas, which is all well and good,
			but how do we calculate the fact that this turf is losing gas to each
			neighbor, at the same rate? Remember, we multiplied each gas mixture
			in the list by the diffusion constant when copying, which means we can't
			just multiply it by (1-(adj_turfs)/8)--it's already multiplied by 1/8!
			So, we use this equation here--(1-(ab))/b turns out to be 1/b-a,
			and since b is GAS_DIFFUSION_CONSTANT, we can calculate that at
			compile-time, then just subtract the adjacent turfs count from the inverse.
			For our 1/8 example, we get 8-3, or 5--this multiplied by the
			already-1/8'thd gas mix, we get 5/8. Easy!
			*/
			let mut end_gas = GasMixture::from_vol(2500.0);
			GasMixtures::with_all_mixtures(|all_mixtures| {
				// NORTH (Byond is +y-up)
				if adj & 1 == 1 {
					if let Some(gas) = gases.get(&(i + max_x)) {
						end_gas.merge(all_mixtures.get(gas.mix).unwrap());
					}
				}
				// SOUTH
				if adj & 2 == 2 {
					if let Some(gas) = gases.get(&(i - max_x)) {
						end_gas.merge(all_mixtures.get(gas.mix).unwrap());
					}
				}
				// EAST
				if adj & 4 == 4 {
					if let Some(gas) = gases.get(&(i + 1)) {
						end_gas.merge(all_mixtures.get(gas.mix).unwrap());
					}
				}
				// WEST
				if adj & 8 == 8 {
					if let Some(gas) = gases.get(&(i - 1)) {
						end_gas.merge(all_mixtures.get(gas.mix).unwrap());
					}
				}
				// UP (I actually don't know if byond is +Z up or down, but Z up is standard, I'm reasonably sure, so.)
				if adj & 16 == 16 {
					if let Some(gas) = gases.get(&(i + (max_y * max_x))) {
						end_gas.merge(all_mixtures.get(gas.mix).unwrap());
					}
				}
				// DOWN
				if adj & 32 == 32 {
					if let Some(gas) = gases.get(&(i - (max_y * max_x))) {
						end_gas.merge(all_mixtures.get(gas.mix).unwrap());
					}
				}
			});
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
			end_gas.multiply(GAS_DIFFUSION_CONSTANT);
			let mut flags: i8 = 0;
			GasMixtures::with_all_mixtures_mut(|all_mixtures| {
				let gas = all_mixtures.get_mut(m.mix).unwrap();
				gas.multiply(1.0 - (GAS_DIFFUSION_CONSTANT * adj_amount as f32));
				end_gas.merge(gas);
				let comparison = end_gas.compare(gas);
				gas.copy_from_mutable(&end_gas);
				if gas.is_visible() {
					flags |= 1;
				}
				if comparison == -2 {
					flags |= 4;
				}
				if gas.can_react() {
					flags |= 2;
				}
			});
			sender.send((*i, flags)).unwrap();
		}
		drop(sender);
	});
	let mut any_err: DMResult = Ok(Value::null());
	use std::collections::HashMap;
	let mut to_cool_down: HashMap<usize, bool> = HashMap::new();
	for (turf_idx, activity_bitflags) in receiver.iter() {
		let turf = TurfGrid::turf_by_id(turf_idx as u32);
		if activity_bitflags & 2 == 2 {
			if let Err(e) = turf.get("air").unwrap().call("react", &[turf.clone()]) {
				any_err = Err(e);
			}
		}
		if activity_bitflags & 1 == 1 {
			to_cool_down.insert(turf_idx, false);
			if let Err(e) = turf.call("update_visuals", &[Value::null()]) {
				any_err = Err(e);
			}
		}
		if activity_bitflags & 4 == 4 {
			to_cool_down.insert(turf_idx, true);
		}
	}
	let res = thread.join();
	rayon::spawn(move || {
		let mut gases = TURF_GASES.write().unwrap();
		for (i, turf_mix) in gases.iter_mut() {
			if let Some(cooling) = to_cool_down.get(i) {
				if *cooling {
					turf_mix.cooldown = turf_mix.next_cooldown;
					turf_mix.next_cooldown *= 2;
				} else {
					turf_mix.cooldown = 0;
					turf_mix.next_cooldown = 1;
				}
			} else {
				turf_mix.cooldown = (turf_mix.cooldown - 1).max(0);
			}
		}
	});
	if res.is_err() {
		Err(runtime!("Atmos turfs thread panicked!"))
	} else {
		any_err
	}
}
