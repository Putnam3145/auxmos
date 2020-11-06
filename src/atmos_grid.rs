use super::gas::gas_mixture::GasMixture;

use super::gas::GasMixtures;

use turf_grid::*;

use dm::*;

use super::gas::constants::*;

use std::collections::VecDeque;

use dashmap::DashMap;

#[derive(Clone, Copy, Default)]
struct TurfMixture {
	/// An index into the thread local gases mix.
	pub mix: usize,
	pub adjacency: i8,
	pub simulation_level: u8,
	pub planetary_atmos: Option<&'static str>,
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
				to_insert.planetary_atmos = Some(&*at_str);
				GasMixtures::with_all_mixtures(|all_mixtures| {
					let entry = PLANETARY_ATMOS
						.entry(to_insert.planetary_atmos.unwrap())
						.or_insert(all_mixtures.get(to_insert.mix).unwrap().clone());
					entry.mark_immutable();
				})
			}
		}
	}
	TURF_GASES
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
		if let Some(turf) = TURF_GASES.get_mut(&id) {
			turf.adjacency = 0;
			for i in 1..adjacent_list.len() + 1 {
				turf.adjacency |= adjacent_list.get(&adjacent_list.get(i)?)?.as_number()? as i8;
			}
		}
	}
	Ok(Value::null())
}

const SIMULATION_LEVEL_NONE: u8 = 0;
const SIMULATION_LEVEL_SHARE_FROM: u8 = 1;
const SIMULATION_LEVEL_SIMULATE: u8 = 2;

use rayon;
use std::sync::mpsc;
use std::time::{Duration, Instant};

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

#[hook("/datum/controller/subsystem/air/proc/process_turfs_extools")]
fn _process_turf_hook() {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature (sleeping turfs).
		It can run in parallel, but doesn't yet. We'll see if it's required for performance reasons.
	*/
	// First we copy the gas list immutably, so we can be sure this is consistent.
	let max_x = TurfGrid::max_x();
	let max_y = TurfGrid::max_y();
	let time_limit = Duration::from_secs_f32(
		args.get(0)
			.ok_or_else(|| runtime!("Wrong number of arguments to process_turfs_extools: 0"))?
			.as_number()?,
	);
	let start_time = Instant::now();
	let (sender, receiver) = mpsc::channel();
	let (gas_sender, gas_receiver) = mpsc::channel();
	rayon::spawn(move || {
		for (i, m) in TURF_GASES
			.into_read_only()
			.iter()
			.filter(|(_, m)| m.simulation_level >= SIMULATION_LEVEL_SIMULATE)
		{
			/*
			We're checking an individual tile now. First, we get the adjacency of this tile. This is
			saved by a turf every single time it gets its adjacent turfs updated, and of course this
			processes everything in one go, blocking the byond thread until it's done (it's called from a hook),
			so it'll be nice and consistent.
			*/
			let adj = m.adjacency;
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
			let adj_tiles = adjacent_tile_ids(adj, *i, max_x, max_y);
			GasMixtures::with_all_mixtures(|all_mixtures| {
				for loc in adj_tiles.iter() {
					if let Some(gas) = TURF_GASES.get(loc) {
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
			// Rather than immediately setting it, we send it to the thread defined below to be set when it's safe to do so.
			gas_sender.send((*i, m.mix, adj, end_gas)).unwrap();
		}
		drop(gas_sender);
	});
	rayon::spawn(move || {
		// This closure is what sets the gas. It locks the mixtures, saves it, then sends the "we did stuff" data to the main thread.
		let write_to_gas = |(i, mix, adj, mut end_gas): (usize, usize, i8, GasMixture)| {
			let mut gas_copy = GasMixture::new(); // so we don't lock the mutex the *entire* time
			let mut comparison = 0;
			let mut flags = 0;
			let adj_amount = adj.count_ones();
			GasMixtures::with_all_mixtures_mut(|all_mixtures| {
				let gas: &mut GasMixture = all_mixtures.get_mut(mix).unwrap();
				gas.multiply(1.0 - (GAS_DIFFUSION_CONSTANT * adj_amount as f32));
				gas.merge(&end_gas);
				gas_copy.copy_from_mutable(gas);
			});
			if gas_copy.is_visible() {
				flags |= 1;
			}
			if gas_copy.can_react() {
				flags |= 2;
			}
			sender.send((i, flags)).unwrap();
		};
		let max_north = TurfGrid::max_x();
		let max_up = TurfGrid::max_y() * max_north;
		/*
			The above two and the below are what ensures the "safety"
			of the operation--it allows for the gas mixtures
			to be set in parallel, and for updating to happen in parallel,
			without causing inconsistent results, by only setting gas mixtures
			that are *definitely not* going to be read again.
		*/
		let mut min_diff = max_north;
		use std::collections::VecDeque;
		let mut chunk_change: VecDeque<(usize, usize, i8, GasMixture)> =
			VecDeque::with_capacity(100);
		for (i, mix, adj, end_gas) in gas_receiver.iter() {
			if adj & 32 == 32 {
				min_diff = max_up;
			}
			chunk_change.push_back((i, mix, adj, end_gas));
			let mut should_process = true;
			while should_process {
				if let Some(front) = chunk_change.front() {
					if let Some(back) = chunk_change.back() {
						if back.0 - front.0 > min_diff as usize {
							write_to_gas(chunk_change.pop_front().unwrap());
						} else {
							should_process = false;
						}
					}
				}
			}
		}
		// The processing thread is done--now we just do the rest.
		for t in chunk_change {
			write_to_gas(t)
		}
	});
	let mut any_err: DMResult = Ok(Value::null());
	let ret_list = List::new();
	for (turf_idx, activity_bitflags) in receiver.iter() {
		let turf = TurfGrid::turf_by_id(turf_idx as u32);
		if start_time.elapsed() > time_limit {
			if activity_bitflags & 3 > 0 {
				if let Err(e) = ret_list.set(&turf, activity_bitflags as f32) {
					any_err = Err(e);
				}
			}
		} else {
			if activity_bitflags & 2 == 2 {
				if let Err(e) = turf.get("air").unwrap().call("react", &[turf.clone()]) {
					any_err = Err(e);
				}
			}
			if activity_bitflags & 1 == 1 {
				if let Err(e) = turf.call("update_visuals", &[Value::null()]) {
					any_err = Err(e);
				}
			}
		}
	}
	if let Err(e) = any_err {
		src.call("stack_trace", &[&Value::from_string(e.message.as_str())])
			.unwrap();
	}
	Ok(Value::from(ret_list))
}

#[derive(Copy, Clone, Default)]
struct MonstermosInfo {
	transfer_dirs: [f32; 7],
	mole_delta: f32,
	curr_transfer_amount: f32,
	distance_score: f32,
	curr_transfer_dir: usize,
	last_slow_queue_cycle: i32,
	fast_done: bool,
	is_planet: bool,
	done_this_cycle: bool,
}

const opp_dir_index: [usize; 7] = [1, 0, 3, 2, 5, 4, 6];

impl MonstermosInfo {
	fn adjust_eq_movement(&mut self, adjacent: &mut Self, dir_index: usize, amount: f32) {
		self.transfer_dirs[dir_index] += amount;
		if dir_index != 6 {
			adjacent.transfer_dirs[opp_dir_index[dir_index]] -= amount;
		}
	}
}

#[hook("/datum/controller/subsystem/air/proc/process_turf_equalize_extools")]
fn _hook_equalize() {
	let monstermos_turf_limit = src.get_number("monstermos_turf_limit")? as usize;
	let monstermos_hard_turf_limit = src.get_number("monstermos_hard_turf_limit")? as usize;
	let time_limit = Duration::from_secs_f32(
		args.get(0)
			.ok_or_else(|| runtime!("Wrong number of arguments to process_turfs_extools: 0"))?
			.as_number()?,
	);
	let start_time = Instant::now();
	let max_x = TurfGrid::max_x();
	let max_y = TurfGrid::max_y();
	let (sender, receiver) = mpsc::channel();
	let (turf_sender, turf_receiver) = mpsc::channel();
	rayon::spawn(|| {
		use std::collections::HashMap;
		let mut info: HashMap<usize, MonstermosInfo> = HashMap::new();
		for (i, m) in TURF_GASES.into_read_only().iter().filter(|(i, m)| {
			if m.simulation_level >= SIMULATION_LEVEL_SIMULATE {
				if let Some(our_info) = info.get(i) {
					if our_info.done_this_cycle {
						return false;
					}
				}
				let adj_tiles = adjacent_tile_ids(m.adjacency, **i, max_x, max_y);
				let mut any_comparison_good = false;
				GasMixtures::with_all_mixtures(|all_mixtures| {
					let our_moles = all_mixtures.get(m.mix).unwrap().total_moles();
					for loc in adj_tiles.iter() {
						if let Some(gas) = TURF_GASES.get(loc) {
							let cmp_gas = all_mixtures.get(gas.mix).unwrap();
							if (cmp_gas.total_moles() - our_moles).abs()
								> MINIMUM_MOLES_DELTA_TO_MOVE
							{
								any_comparison_good = true;
								return;
							}
						}
					}
				});
				return any_comparison_good;
			}
			false
		}) {
			let mut turfs: Vec<(usize, TurfMixture)> = Vec::new();
			let mut border_turfs: VecDeque<(usize, TurfMixture)> = VecDeque::new();
			let mut planet_turfs: Vec<(usize, TurfMixture)> = Vec::new();
			let mut total_moles: f32 = 0.0;
			border_turfs.push_back((*i, *m));
			while border_turfs.len() > 0 {
				if turfs.len() > monstermos_hard_turf_limit {
					break;
				}
				let (cur_idx, cur_turf) = border_turfs.pop_front().unwrap();
				let cur_info = info.entry(cur_idx).or_insert(Default::default());
				cur_info.distance_score = 0.0;
				if turfs.len() < monstermos_turf_limit {
					let mut turf_moles = 0.0;
					GasMixtures::with_all_mixtures(|all_mixtures| {
						turf_moles = all_mixtures.get(cur_turf.mix).unwrap().total_moles();
					});
					cur_info.mole_delta = turf_moles;
					if cur_turf.planetary_atmos.is_some() {
						planet_turfs.push((cur_idx,cur_turf));
						continue;
					}
					total_moles += turf_moles;
				}
				for loc in adjacent_tile_ids(cur_turf.adjacency, cur_idx, max_x, max_y).iter() {
					let adj_turf = TURF_GASES.get(loc).unwrap();
					let adj_info = info.entry(cur_idx).or_insert(Default::default());
					if !adj_info.done_this_cycle && adj_turf.simulation_level != SIMULATION_LEVEL_NONE {
						border_turfs.push_back((*loc, *adj_turf.value()));
					} /* else { // this is in C++, copy+pasted directly, don't just uncomment it you dink
						 // Uh oh! looks like someone opened an airlock to space! TIME TO SUCK ALL THE AIR OUT!!!
						 // NOT ONE OF YOU IS GONNA SURVIVE THIS
						 // (I just made explosions less laggy, you're welcome)
						 //turfs.push_back(adj);
						 //explosively_depressurize(cyclenum);
						 //return;
					 }*/
				}
				turfs.push((cur_idx,cur_turf));
			}
			if turfs.len() > monstermos_turf_limit {
				for i in monstermos_turf_limit..turfs.len() {
					let (idx, m) = turfs[i];
					info.entry(idx)
						.or_insert(Default::default())
						.done_this_cycle = false;
				}
			}
			let average_moles = total_moles / (turfs.len() as f32);
			let giver_turfs = Vec::new();
			let taker_turfs = Vec::new();
			for (i, m) in turfs.iter() {
				let mut cur_info = info.entry(*i).or_insert(Default::default());
				cur_info.mole_delta -= average_moles;
				if cur_info.mole_delta > 0.0 {
					giver_turfs.push((i, m));
				} else {
					taker_turfs.push((i, m));
				}
			}
			let log_n = ((turfs.len() as f32).log2().floor()) as usize;
			if giver_turfs.len() > log_n && taker_turfs.len() > log_n {
				use float_ord::FloatOrd;
				turfs.sort_by_cached_key(|(idx, turf)| FloatOrd(info.get(idx).unwrap().mole_delta) );
				for (i, m) in turfs.iter() {
					let mut cur_info = info.get(i).unwrap();
					cur_info.fast_done = true;
					let mut eligible_adjacents = 0;
					if cur_info.mole_delta > 0.0 {
						for j in 0..6 {
							let bit = 1 << j;
							if m.adjacency & bit == bit {
								let loc = adjacent_tile_id(j, *i, max_x, max_y);
								if let Some(adj_info) = info.get(&loc) {
									if adj_info.fast_done || adj_info.done_this_cycle {
										continue;
									} else {
										eligible_adjacents |= bit;
									}
								}
							}
						}
						let amt_eligible = eligible_adjacents.count_ones();
						let moles_to_move = cur_info.mole_delta / amt_eligible as f32;
						for j in 0..6 {
							let bit = 1 << j;
							if eligible_adjacents & bit == bit {
								let mut adj_turf =
									info.get_mut(&adjacent_tile_id(j, *i, max_x, max_y)).unwrap();
								cur_info.adjust_eq_movement(adj_turf, j as usize, moles_to_move);
								cur_info.mole_delta -= moles_to_move;
								adj_turf.mole_delta += moles_to_move;
							}
						}
					}
				}
				giver_turfs.clear();
				taker_turfs.clear();
				for (i, m) in turfs.iter() {
					let mut cur_info = info.entry(*i).or_insert(Default::default());
					if cur_info.mole_delta > 0.0 {
						giver_turfs.push((i, m));
					} else {
						taker_turfs.push((i, m));
					}
				}
			}
			// alright this is the part that can become O(n^2).
			if giver_turfs.len() < taker_turfs.len() {
				// as an optimization, we choose one of two methods based on which list is smaller. We really want to avoid O(n^2) if we can.
				let mut queue: VecDeque<(i, TurfMixture)> =
					VecDeque::with_capacity(taker_turfs.len());
				let mut queue_cycle_slow = 0;
				for (i, m) in giver_turfs.iter() {
					let mut giver_info = info.get(i).unwrap();
					giver_info.curr_transfer_dir = 6;
					giver_info.curr_transfer_amount = 0.0;
					queue_cycle_slow = queue_cycle_slow + 1;
					queue.clear();
					queue.push_front((i, m));
					let mut queue_idx = 0;
					while queue_idx < queue.len() {
						if giver_info.mole_delta <= 0.0 {
							break;
						}
						let (idx, mut turf) = queue.get(queue_idx).unwrap().copy();
						for j in 0..6 {
							let bit = 1 << j;
							if turf.adjacency & bit {
								if let Some(mut adj_info) =
									info.get_mut(&adjacent_tile_id(j, idx, max_x, max_y))
								{
									if cur_info.mole_delta <= 0 {
										break;
									}
									if !adj_info.done_this_cycle
										&& adj_info.last_slow_queue_cycle != queue_cycle_slow
									{
										queue.push_back((i, *TURF_GASES.get(i).unwrap().value()));
										adj_info.last_slow_queue_cycle = queue_cycle_slow;
										adj_info.curr_transfer_dir = opp_dir_index[j as usize];
										adj_info.curr_transfer_amount = 0.0;
										if adj_info.mole_delta < 0.0 {
											if -adj_info.mole_delta > giver_info.mole_delta {
												adj_info.curr_transfer_amount -=
													giver_info.mole_delta;
												adj_info.mole_delta += giver_info.mole_delta;
												giver_info.mole_delta = 0.0;
											} else {
												adj_info.curr_transfer_amount +=
													adj_info.mole_delta;
												giver_info.mole_delta += adj_info.mole_delta;
												adj_info.mole_delta = 0.0;
											}
										}
									}
								}
							}
						}
					}
					for (idx, turf) in queue.drain() {
						let mut turf_info = info.get_mut(idx).unwrap();
						if turf_info.curr_transfer_amount > 0.0 && turf_info.curr_transfer_dir != 6
						{
							let mut adj_info = info.get_mut(&(turf_info.curr_transfer_dir as usize)).unwrap();
							turf_info.adjust_eq_movement(
								adj_info,
								turf_info.curr_transfer_dir,
								turf_info.curr_transfer_amount,
							);
							adj_info.curr_transfer_amount += turf_info.curr_transfer_amount;
							turf_info.curr_transfer_amount = 0.0;
						}
					}
				}
			} else {
				let mut queue: VecDeque<(i, TurfMixture)> =
					VecDeque::with_capacity(giver_turfs.len());
				let mut queue_cycle_slow = 0;
				for (i, m) in taker_turfs.iter() {
					let mut taker_info = info.get(i).unwrap();
					taker_info.curr_transfer_dir = 6;
					taker_info.curr_transfer_amount = 0.0;
					queue_cycle_slow = queue_cycle_slow + 1;
					queue.clear();
					queue.push_front((i, m));
					let mut queue_idx = 0;
					while queue_idx < queue.len() {
						if taker_info.mole_delta >= 0.0 {
							break;
						}
						let (idx, mut turf) = queue.get(queue_idx).unwrap().copy();
						for j in 0..6 {
							let bit = 1 << j;
							if turf.adjacency & bit {
								if let Some(mut adj_info) =
									info.get_mut(&adjacent_tile_id(j, idx, max_x, max_y))
								{
									if taker_info.mole_delta >= 0.0 {
										break;
									}
									if !adj_info.done_this_cycle
										&& adj_info.last_slow_queue_cycle != queue_cycle_slow
									{
										queue.push_back((i, *TURF_GASES.get(i).unwrap().value()));
										adj_info.last_slow_queue_cycle = queue_cycle_slow;
										adj_info.curr_transfer_dir = opp_dir_index[j as usize];
										adj_info.curr_transfer_amount = 0.0;
										if adj_info.mole_delta > 0.0 {
											if adj_info.mole_delta > -taker_info.mole_delta {
												adj_info.curr_transfer_amount -=
													taker_info.mole_delta;
												adj_info.mole_delta += taker_info.mole_delta;
												taker_info.mole_delta = 0.0;
											} else {
												adj_info.curr_transfer_amount +=
													adj_info.mole_delta;
												taker_info.mole_delta += adj_info.mole_delta;
												adj_info.mole_delta = 0.0;
											}
										}
									}
								}
							}
						}
					}
					for (idx, turf) in queue.drain() {
						let mut turf_info = info.get_mut(idx).unwrap();
						if turf_info.curr_transfer_amount > 0.0 && turf_info.curr_transfer_dir != 6
						{
							let mut adj_info = info.get_mut(&(turf_info.curr_transfer_dir as usize)).unwrap();
							turf_info.adjust_eq_movement(
								adj_info,
								turf_info.curr_transfer_dir,
								turf_info.curr_transfer_amount,
							);
							adj_info.curr_transfer_amount += turf_info.curr_transfer_amount;
							turf_info.curr_transfer_amount = 0.0;
						}
					}
				}
			}
			turf_sender
				.send(turfs.iter().map(|(i, m)| (i, (m, *info.get(i).unwrap()))).collect())
				.unwrap();
		}
		drop(turf_sender);
	});
	rayon::spawn(|| {
		let finalize_eq_neighbors;
		let finalize_eq = |i, turf, monstermos_info, other_turfs| {
			let transfer_dirs = monstermos.transfer_dirs.copy();
			monstermos.transfer_dirs.iter_mut().for_each(|m| *m = 0.0);
			let planet_transfer_amount = transfer_dirs[6];
			let mut needs_eq_neighbors = false;
			if planet_transfer_amount > 0.0 {
				GasMixtures::with_all_mixtures(|all_mixtures| {
					let air = all_mixtures.get(turf.mix).unwrap();
					if air.total_moles() < planet_transfer_amount {
						needs_eq_neighbors = true;
					}
				});
				if needs_eq_neighbors {
					finalize_eq_neighbors(i, turf, monstermos_info, other_turfs);
				}
				GasMixtures::with_all_mixtures_mut(|all_mixtures| {
					let air = all_mixtures.get_mut(turf.mix).unwrap();
					air.remove(planet_transfer_amount);
				});
			} else if planet_transfer_amount < 0.0 {
				if let Some(planet_air) = PLANETARY_ATMOS.get(turf.planetary_atmos) {
					let planet_sum = planet_air.total_moles();
					if planet_sum > 0.0 {
						GasMixtures::with_all_mixtures_mut(|all_mixtures| {
							let air = all_mixtures.get_mut(turf.mix).unwrap();
							air.merge(planet_air * (-planet_transfer_amount / planet_sum));
						});
					}
				}
			}
			for j in 0..6 {
				let bit = j << 1;
				if turf.adjacency & bit == bit {
					let amount = monstermos_info.transfer_dirs[j];
					if amount > 0.0 {
						GasMixtures::with_all_mixtures(|all_mixtures| {
							let air = all_mixtures.get(turf.mix).unwrap();
							if air.total_moles() < amount {
								needs_eq_neighbors = true;
							}
						});
						if needs_eq_neighbors {
							finalize_eq_neighbors(i, turf, monstermos_info, other_turfs);
						}
						let adj_id = adjacent_tile_id(j, i, max_x, max_y);
						let (adj_turf, &mut adj_info) = other_turfs.get_mut(adj_id).unwrap();
						adj_info.transfer_dirs[opp_dir_index[j as usize]] = 0.0;
						GasMixtures::with_all_mixtures_mut(|all_mixtures| {
							let air = all_mixtures.get(turf.mix).unwrap();
							let other_air = all_mixtures.get_mut(adj_turf.mix).unwrap();
							other_air.merge(air.remove(amount));
						});
						sender.send((i, adj_id, amount));
					}
				}
			}
		};
		finalize_eq_neighbors = |i, turf, monstermos_info, other_turfs| {
			for j in 0..6 {
				let amount = monstermos_info.transfer_dirs[i];
				let bit = 1 << j;
				if amount < 0.0 && turf.adjacency & bit == bit {
					let adjacent_id = adjacent_tile_id(j, i, max_x, max_y);
					if let (other_turf, other_info) = other_turfs.get(adjacent_id) {
						finalize_eq(adjacent_id, other_turf, other_info, other_turfs);
					}
				}
			}
		};
		for turf_set in turf_receiver.recv() {
			for (i, (turf, monstermos_info)) in turf_set.iter_mut() {
				finalize_eq(i, turf, monstermos_info, turf_set);
			}
		}
		drop(sender);
	});
	let mut any_err: DMResult = Ok(Value::null());
	let ret_list = List::new();
	for (turf_idx, other_idx, amount) in receiver.iter() {
		let real_amount = Value::from(amount);
		let turf = TurfGrid::turf_by_id(turf_idx as u32);
		let other_turf = TurfGrid::turf_by_id(other_idx as u32);
		if start_time.elapsed() > time_limit {
			let mut new_list = List::new();
			new_list.append(&other_turf);
			new_list.append(&real_amount);
			if let Err(e) = ret_list.set(&turf, &Value::from(new_list)) {
				any_err = Err(e);
			}
		} else {
			if let Err(e) = turf.call("update_visuals", &[Value::null()]) {
				any_err = Err(e);
			}
			if let Err(e) = other_turf.call("update_visuals", &[Value::null()]) {
				any_err = Err(e);
			}
			if let Err(e) = turf.call("consider_pressure_difference", &[other_turf, real_amount]) {
				any_err = Err(e);
			}
		}
	}
	if let Err(e) = any_err {
		src.call("stack_trace", &[&Value::from_string(e.message.as_str())])
			.unwrap();
	}
	Ok(Value::from(ret_list))
}
