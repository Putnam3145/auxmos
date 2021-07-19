use super::*;

use std::collections::VecDeque;

use std::collections::{BTreeMap, BTreeSet};

use auxcallback::byond_callback_sender;

use std::cell::Cell;

type TransferInfo = [f32; 7];

type MixWithID = (TurfID, TurfMixture);

#[derive(Copy, Clone, Default)]
struct MonstermosInfo {
	transfer_dirs: TransferInfo,
	mole_delta: f32,
	curr_transfer_amount: f32,
	curr_transfer_dir: usize,
	last_slow_queue_cycle: i32,
	fast_done: bool,
}

const OPP_DIR_INDEX: [usize; 7] = [1, 0, 3, 2, 5, 4, 6];

impl MonstermosInfo {
	fn adjust_eq_movement(&mut self, adjacent: &mut Self, dir_index: usize, amount: f32) {
		self.transfer_dirs[dir_index] += amount;
		if dir_index != 6 {
			adjacent.transfer_dirs[OPP_DIR_INDEX[dir_index]] -= amount;
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_eq_movement() {
		let mut info_a: MonstermosInfo = Default::default();
		let mut info_b: MonstermosInfo = Default::default();
		info_a.adjust_eq_movement(&mut info_b, 1, 5.0);
		assert_eq!(info_a.transfer_dirs[1], 5.0);
		assert_eq!(info_b.transfer_dirs[0], -5.0);
	}
}

fn finalize_eq(
	i: TurfID,
	turf: &TurfMixture,
	info: &BTreeMap<TurfID, Cell<MonstermosInfo>>,
	max_x: i32,
	max_y: i32,
) {
	let sender = byond_callback_sender();
	let monstermos_orig = info.get(&i).unwrap();
	let transfer_dirs = monstermos_orig.get().transfer_dirs;
	let planet_transfer_amount = transfer_dirs[6];
	if planet_transfer_amount > 0.0 {
		if turf.total_moles() < planet_transfer_amount {
			finalize_eq_neighbors(i, turf, info, max_x, max_y);
		}
		GasArena::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(turf.mix)
				.unwrap()
				.write()
				.remove(planet_transfer_amount);
		})
	} else if planet_transfer_amount < 0.0 {
		if let Some(air_entry) = turf.planetary_atmos.and_then(|i| planetary_atmos().get(&i)) {
			let planet_air = air_entry.value();
			let planet_sum = planet_air.total_moles();
			if planet_sum > 0.0 {
				GasArena::with_all_mixtures(|all_mixtures| {
					all_mixtures
						.get(turf.mix)
						.unwrap()
						.write()
						.merge(&(planet_air * (-planet_transfer_amount / planet_sum)));
				});
			}
		}
	}
	for (j, adj_id) in adjacent_tile_ids(turf.adjacency, i, max_x, max_y) {
		let amount = transfer_dirs[j as usize];
		if amount > 0.0 {
			if turf.total_moles() < amount {
				finalize_eq_neighbors(i, turf, info, max_x, max_y);
			}
			if let Some(adj_orig) = info.get(&adj_id) {
				if let Some(adj_turf) = turf_gases().get(&adj_id) {
					let mut adj_info = adj_orig.get();
					adj_info.transfer_dirs[OPP_DIR_INDEX[j as usize]] = 0.0;
					if turf.mix != adj_turf.mix {
						GasArena::with_all_mixtures(|all_mixtures| {
							let our_entry = all_mixtures.get(turf.mix).unwrap();
							let their_entry = all_mixtures.get(adj_turf.mix).unwrap();
							let mut air = our_entry.write();
							let mut other_air = their_entry.write();
							other_air.merge(&air.remove(amount));
						});
					}
					adj_orig.set(adj_info);
					sender
						.send(Box::new(move || {
							let real_amount = Value::from(-amount);
							let turf = unsafe { Value::turf_by_id_unchecked(i as u32) };
							let other_turf = unsafe { Value::turf_by_id_unchecked(adj_id as u32) };
							if let Err(e) = turf
								.call("consider_pressure_difference", &[&other_turf, &real_amount])
							{
								turf.call(
									"stack_trace",
									&[&Value::from_string(e.message.as_str())?],
								)
								.unwrap();
							}
							Ok(Value::null())
						}))
						.unwrap();
				}
			}
		}
	}
}

fn finalize_eq_neighbors(
	i: TurfID,
	turf: &TurfMixture,
	info: &BTreeMap<TurfID, Cell<MonstermosInfo>>,
	max_x: i32,
	max_y: i32,
) {
	let transfer_dirs = info.get(&i).unwrap().get().transfer_dirs;
	for (j, adjacent_id) in adjacent_tile_ids(turf.adjacency, i, max_x, max_y) {
		let amount = transfer_dirs[j as usize];
		if amount < 0.0 {
			if let Some(other_turf) = turf_gases().get(&adjacent_id) {
				finalize_eq(adjacent_id, other_turf.value(), info, max_x, max_y);
			}
		}
	}
}

#[deprecated(
	note = "It panics when trying to get the 'pressure_direction' string from byond. I don't even know, man, it's cursed."
)]
#[cfg(feature = "explosive_decompression")]
fn explosively_depressurize(
	turf_idx: TurfID,
	turf: TurfMixture,
	mut found_turfs: BTreeSet<TurfID>,
	equalize_hard_turf_limit: usize,
	max_x: i32,
	max_y: i32,
) -> DMResult {
	let mut info: BTreeMap<TurfID, Cell<MonstermosInfo>> = BTreeMap::new();
	let mut turfs: Vec<MixWithID> = Vec::new();
	let mut space_turfs: Vec<MixWithID> = Vec::new();
	turfs.push((turf_idx, turf));
	let cur_orig = info.get(&turf_idx).unwrap();
	let mut cur_info: MonstermosInfo = Default::default();
	found_turfs.insert(turf_idx);
	cur_info.curr_transfer_dir = 6;
	cur_orig.set(cur_info);
	let mut warned_about_planet_atmos = false;
	let mut cur_queue_idx = 0;
	while cur_queue_idx < turfs.len() {
		let (i, m) = turfs[cur_queue_idx];
		cur_queue_idx += 1;
		let cur_orig = info.get(&i).unwrap();
		let mut cur_info = cur_orig.get();
		found_turfs.insert(i);
		cur_info.curr_transfer_dir = 6;
		cur_orig.set(cur_info);
		if m.planetary_atmos.is_some() {
			warned_about_planet_atmos = true;
			continue;
		}
		if m.is_immutable() {
			space_turfs.push((i, m));
			unsafe { Value::turf_by_id_unchecked(i) }
				.set(byond_string!("pressure_specific_target"), &unsafe {
					Value::turf_by_id_unchecked(i)
				})?;
		} else {
			if cur_queue_idx > equalize_hard_turf_limit {
				continue;
			}
			for j in 0..6 {
				let bit = 1 << j;
				if m.adjacency & bit == bit {
					let loc = adjacent_tile_id(j as u8, i, max_x, max_y);
					if found_turfs.contains(&loc) {
						continue;
					}
					if let Some(adj) = turf_gases().get(&loc) {
						let (&adj_i, &adj_m) = (adj.key(), adj.value());
						unsafe { Value::turf_by_id_unchecked(i) }.call(
							"consider_firelocks",
							&[&unsafe { Value::turf_by_id_unchecked(loc) }],
						)?;
						let new_m = turf_gases().get(&i).unwrap();
						if new_m.adjacency & bit == bit {
							found_turfs.insert(loc);
							info.entry(loc).or_default().take();
							turfs.push((adj_i, adj_m));
						}
					}
				}
			}
		}
		if warned_about_planet_atmos {
			return Ok(Value::null()); // planet atmos > space
		}
	}
	let mut progression_order: Vec<MixWithID> = Vec::with_capacity(space_turfs.len());
	for (i, m) in space_turfs.iter() {
		progression_order.push((*i, *m));
		let cur_info = info.get_mut(i).unwrap().get_mut();
		cur_info.curr_transfer_dir = 6;
	}
	cur_queue_idx = 0;
	while cur_queue_idx < progression_order.len() {
		let (i, m) = progression_order[cur_queue_idx];
		for (j, loc) in adjacent_tile_ids(m.adjacency, i, max_x, max_y) {
			if let Some(adj) = turf_gases().get(&loc) {
				let (adj_i, adj_m) = (adj.key(), adj.value());
				let adj_orig = info.entry(loc).or_default();
				let mut adj_info = adj_orig.get();
				if !adj_m.is_immutable() {
					adj_info.curr_transfer_dir = OPP_DIR_INDEX[j as usize];
					adj_info.curr_transfer_amount = 0.0;
					unsafe { Value::turf_by_id_unchecked(*adj_i) }
						.set(byond_string!("pressure_specific_target"), &unsafe {
							Value::turf_by_id_unchecked(i)
						})?;
					adj_orig.set(adj_info);
					progression_order.push((*adj_i, *adj_m));
				}
			}
			cur_queue_idx += 1;
		}
	}
	for (i, m) in progression_order.iter().rev() {
		let cur_orig = info.get(i).unwrap();
		let mut cur_info = cur_orig.get();
		if cur_info.curr_transfer_dir == 6 {
			continue;
		}
		let loc = adjacent_tile_id(cur_info.curr_transfer_dir as u8, *i, max_x, max_y);
		if let Some(adj) = turf_gases().get(&loc) {
			let (adj_i, adj_m) = (adj.key(), adj.value());
			let adj_orig = info.get(&loc).unwrap();
			let mut adj_info = adj_orig.get();
			let sum = adj_m.total_moles();
			cur_info.curr_transfer_amount += sum;
			adj_info.curr_transfer_amount += cur_info.curr_transfer_amount;
			if adj_info.curr_transfer_dir == 6 {
				let byond_turf = unsafe { Value::turf_by_id_unchecked(*adj_i) };
				byond_turf.set(
					byond_string!("pressure_difference"),
					cur_info.curr_transfer_amount,
				)?;
				byond_turf.set(
					byond_string!("pressure_direction"),
					(1 << cur_info.curr_transfer_dir) as f32,
				)?;
			}
			m.clear_air();
			let byond_turf = unsafe { Value::turf_by_id_unchecked(*i) };
			byond_turf.set(
				byond_string!("pressure_difference"),
				cur_info.curr_transfer_amount,
			)?;
			byond_turf.set(
				byond_string!("pressure_direction"),
				(1 << cur_info.curr_transfer_dir) as f32,
			)?;
			byond_turf.call("handle decompression floor rip", &[&Value::from(sum)])?;
		}
	}
	Ok(Value::null())
	//	if (total_gases_deleted / turfs.len() as f32) > 20.0 && turfs.len() > 10 { // logging I guess
	//	}
}

fn flood_fill_equalize_turfs(
	i: TurfID,
	m: TurfMixture,
	equalize_turf_limit: usize,
	equalize_hard_turf_limit: usize,
	max_x: i32,
	max_y: i32,
	found_turfs: &mut BTreeSet<TurfID>,
	info: &mut BTreeMap<TurfID, Cell<MonstermosInfo>>,
) -> Option<(Vec<MixWithID>, Vec<MixWithID>, f64)> {
	let mut turfs: Vec<MixWithID> = Vec::with_capacity(equalize_hard_turf_limit);
	let mut border_turfs: VecDeque<MixWithID> = VecDeque::with_capacity(equalize_turf_limit);
	let mut planet_turfs: Vec<MixWithID> = Vec::new();
	#[cfg(feature = "explosive_decompression")]
	let sender = byond_callback_sender();
	let mut total_moles = 0.0_f64;
	border_turfs.push_back((i, m));
	#[allow(unused_mut)]
	let mut space_this_time = false;
	loop {
		if turfs.len() > equalize_hard_turf_limit {
			break;
		}
		if let Some((cur_idx, cur_turf)) = border_turfs.pop_front() {
			found_turfs.insert(cur_idx);
			if turfs.len() < equalize_turf_limit {
				if cur_turf.planetary_atmos.is_some() {
					planet_turfs.push((cur_idx, cur_turf));
					continue;
				}
				total_moles += cur_turf.total_moles() as f64;
			}
			for (_, loc) in adjacent_tile_ids(cur_turf.adjacency, cur_idx, max_x, max_y) {
				if found_turfs.contains(&loc) {
					continue;
				}
				found_turfs.insert(loc);
				if let Some(adj_turf) = turf_gases().get(&loc) {
					let adj_orig = info.entry(cur_idx).or_default();
					#[cfg(feature = "explosive_decompression")]
					{
						adj_orig.take();
						border_turfs.push_back((loc, *adj_turf.value()));
						if adj_turf.value().is_immutable() {
							// Uh oh! looks like someone opened an airlock to space! TIME TO SUCK ALL THE AIR OUT!!!
							// NOT ONE OF YOU IS GONNA SURVIVE THIS
							// (I just made explosions less laggy, you're welcome)
							turfs.push((loc, *adj_turf.value()));
							let map_copy = found_turfs.clone();
							let turf_copy = *m;
							let _ = sender.send(Box::new(move || {
								explosively_depressurize(
									i,
									turf_copy,
									map_copy.clone(),
									equalize_hard_turf_limit,
									max_x,
									max_y,
								)
							}));
							space_this_time = true;
						}
					}
					#[cfg(not(feature = "explosive_decompression"))]
					{
						if adj_turf.enabled() {
							adj_orig.take();
							border_turfs.push_back((loc, *adj_turf.value()));
						}
					}
				}
				if space_this_time {
					break;
				}
			}
			turfs.push((cur_idx, cur_turf));
		} else {
			break;
		}
	}
	(!space_this_time).then(|| (turfs, planet_turfs, total_moles))
}

fn partition_turfs(
	turfs: &Vec<MixWithID>,
	info: &mut BTreeMap<TurfID, Cell<MonstermosInfo>>,
	average_moles: f32,
) -> (Vec<MixWithID>, Vec<MixWithID>) {
	turfs.iter().partition(|&(i, m)| {
		let cur_info = info.entry(*i).or_default().get_mut();
		cur_info.mole_delta = m.total_moles() - average_moles;
		cur_info.mole_delta > 0.0
	})
}

fn monstermos_fast_process(
	i: TurfID,
	m: TurfMixture,
	max_x: i32,
	max_y: i32,
	info: &mut BTreeMap<TurfID, Cell<MonstermosInfo>>,
) {
	let cur_orig = info.get(&i).unwrap();
	let mut cur_info = cur_orig.get();
	cur_info.fast_done = true;
	let mut eligible_adjacents: i32 = 0;
	if cur_info.mole_delta > 0.0 {
		for (j, loc) in adjacent_tile_ids(m.adjacency, i, max_x, max_y) {
			if let Some(adj_orig) = info.get(&loc) {
				let adj_info = adj_orig.get();
				if !adj_info.fast_done {
					eligible_adjacents |= 1 << j;
				}
			}
		}
		let amt_eligible = eligible_adjacents.count_ones();
		if amt_eligible == 0 {
			cur_orig.set(cur_info);
			return;
		}
		let moles_to_move = cur_info.mole_delta / amt_eligible as f32;
		for (j, loc) in adjacent_tile_ids(eligible_adjacents as u8, i, max_x, max_y) {
			let adj_orig = info.get(&loc).unwrap();
			let mut adj_info = adj_orig.get();
			cur_info.adjust_eq_movement(&mut adj_info, j as usize, moles_to_move);
			cur_info.mole_delta -= moles_to_move;
			adj_info.mole_delta += moles_to_move;
			adj_orig.set(adj_info);
		}
	}
	cur_orig.set(cur_info);
}

fn give_to_takers(
	giver_turfs: &Vec<MixWithID>,
	taker_turfs: &Vec<MixWithID>,
	max_x: i32,
	max_y: i32,
	info: &BTreeMap<TurfID, Cell<MonstermosInfo>>,
	queue_cycle_slow: &mut i32,
) {
	let mut queue: Vec<MixWithID> = Vec::with_capacity(taker_turfs.len());
	for (i, m) in giver_turfs {
		let giver_orig = info.get(i).unwrap();
		let mut giver_info = giver_orig.get();
		giver_info.curr_transfer_dir = 6;
		giver_info.curr_transfer_amount = 0.0;
		*queue_cycle_slow += 1;
		queue.clear();
		queue.push((*i, *m));
		giver_info.last_slow_queue_cycle = *queue_cycle_slow;
		let mut queue_idx = 0;
		while queue_idx < queue.len() {
			if giver_info.mole_delta <= 0.0 {
				break;
			}
			let (idx, turf) = *queue.get(queue_idx).unwrap();
			for (j, loc) in adjacent_tile_ids(turf.adjacency, idx, max_x, max_y) {
				if let Some(adj_orig) = info.get(&loc) {
					if let Some(adj_mix) = turf_gases().get(&loc) {
						if giver_info.mole_delta <= 0.0 {
							break;
						}
						let mut adj_info = adj_orig.get();
						if adj_info.last_slow_queue_cycle != *queue_cycle_slow {
							queue.push((loc, *adj_mix.value()));
							adj_info.last_slow_queue_cycle = *queue_cycle_slow;
							adj_info.curr_transfer_dir = OPP_DIR_INDEX[j as usize];
							adj_info.curr_transfer_amount = 0.0;
							if adj_info.mole_delta < 0.0 {
								if -adj_info.mole_delta > giver_info.mole_delta {
									adj_info.curr_transfer_amount -= giver_info.mole_delta;
									adj_info.mole_delta += giver_info.mole_delta;
									giver_info.mole_delta = 0.0;
								} else {
									adj_info.curr_transfer_amount += adj_info.mole_delta;
									giver_info.mole_delta += adj_info.mole_delta;
									adj_info.mole_delta = 0.0;
								}
							}
							adj_orig.set(adj_info);
							giver_orig.set(giver_info);
						}
					}
				}
			}
			queue_idx += 1;
		}
		for (idx, _) in queue.drain(..) {
			let turf_orig = info.get(&idx).unwrap();
			let mut turf_info = turf_orig.get();
			if turf_info.curr_transfer_amount != 0.0 && turf_info.curr_transfer_dir != 6 {
				let adj_tile_id =
					adjacent_tile_id(turf_info.curr_transfer_dir as u8, idx, max_x, max_y);
				let adj_orig = info.get(&adj_tile_id).unwrap();
				let mut adj_info = adj_orig.get();
				turf_info.adjust_eq_movement(
					&mut adj_info,
					turf_info.curr_transfer_dir,
					turf_info.curr_transfer_amount,
				);
				adj_info.curr_transfer_amount += turf_info.curr_transfer_amount;
				turf_info.curr_transfer_amount = 0.0;
				turf_orig.set(turf_info);
				adj_orig.set(adj_info);
			}
		}
		giver_orig.set(giver_info);
	}
}

fn take_from_givers(
	taker_turfs: &Vec<MixWithID>,
	giver_turfs: &Vec<MixWithID>,
	max_x: i32,
	max_y: i32,
	info: &BTreeMap<TurfID, Cell<MonstermosInfo>>,
	queue_cycle_slow: &mut i32,
) {
	let mut queue: Vec<MixWithID> = Vec::with_capacity(giver_turfs.len());
	for (i, m) in taker_turfs {
		let taker_orig = info.get(i).unwrap();
		let mut taker_info = taker_orig.get();
		taker_info.curr_transfer_dir = 6;
		taker_info.curr_transfer_amount = 0.0;
		*queue_cycle_slow += 1;
		queue.clear();
		queue.push((*i, *m));
		taker_info.last_slow_queue_cycle = *queue_cycle_slow;
		let mut queue_idx = 0;
		while queue_idx < queue.len() {
			if taker_info.mole_delta >= 0.0 {
				break;
			}
			let (idx, turf) = *queue.get(queue_idx).unwrap();
			for (j, loc) in adjacent_tile_ids(turf.adjacency, idx, max_x, max_y) {
				if let Some(adj_orig) = info.get(&loc) {
					if let Some(adj_mix) = turf_gases().get(&loc) {
						let mut adj_info = adj_orig.get();
						if taker_info.mole_delta >= 0.0 {
							break;
						}
						if adj_info.last_slow_queue_cycle != *queue_cycle_slow {
							queue.push((loc, *adj_mix));
							adj_info.last_slow_queue_cycle = *queue_cycle_slow;
							adj_info.curr_transfer_dir = OPP_DIR_INDEX[j as usize];
							adj_info.curr_transfer_amount = 0.0;
							if adj_info.mole_delta > 0.0 {
								if adj_info.mole_delta > -taker_info.mole_delta {
									adj_info.curr_transfer_amount -= taker_info.mole_delta;
									adj_info.mole_delta += taker_info.mole_delta;
									taker_info.mole_delta = 0.0;
								} else {
									adj_info.curr_transfer_amount += adj_info.mole_delta;
									taker_info.mole_delta += adj_info.mole_delta;
									adj_info.mole_delta = 0.0;
								}
							}
							adj_orig.set(adj_info);
							taker_orig.set(taker_info);
						}
					}
				}
			}
			queue_idx += 1;
		}
		for (idx, _) in queue.drain(..) {
			let turf_orig = info.get(&idx).unwrap();
			let mut turf_info = turf_orig.get();
			if turf_info.curr_transfer_amount != 0.0 && turf_info.curr_transfer_dir != 6 {
				let adj_orig = info
					.get(&adjacent_tile_id(
						turf_info.curr_transfer_dir as u8,
						idx,
						max_x,
						max_y,
					))
					.unwrap();
				let mut adj_info = adj_orig.get();
				turf_info.adjust_eq_movement(
					&mut adj_info,
					turf_info.curr_transfer_dir,
					turf_info.curr_transfer_amount,
				);
				adj_info.curr_transfer_amount += turf_info.curr_transfer_amount;
				turf_info.curr_transfer_amount = 0.0;
				turf_orig.set(turf_info);
				adj_orig.set(adj_info);
			}
		}
	}
}

fn process_planet_turfs(
	planet_turfs: &Vec<MixWithID>,
	average_moles: f32,
	max_x: i32,
	max_y: i32,
	info: &mut BTreeMap<TurfID, Cell<MonstermosInfo>>,
	mut queue_cycle_slow: i32,
) -> DMResult {
	let (_, sample_turf) = planet_turfs[0];
	let planet_sum = planetary_atmos()
		.get(&sample_turf.planetary_atmos.unwrap())
		.unwrap()
		.value()
		.total_moles();
	let target_delta = planet_sum - average_moles;
	queue_cycle_slow += 1;
	let mut progression_order: Vec<MixWithID> = Vec::with_capacity(planet_turfs.len());
	for (i, m) in planet_turfs.iter() {
		progression_order.push((*i, *m));
		let mut cur_info = info.entry(*i).or_default().get_mut();
		cur_info.curr_transfer_dir = 6;
		cur_info.last_slow_queue_cycle = queue_cycle_slow;
	}
	// now build a map of where the path to a planet turf is for each tile.
	let mut queue_idx = 0;
	while queue_idx < progression_order.len() {
		let (i, m) = progression_order[queue_idx];
		for j in 0..6 {
			let bit = 1 << j;
			if m.adjacency & bit == bit {
				let loc = adjacent_tile_id(j, i, max_x, max_y);
				if let Some(adj_orig) = info.get(&loc) {
					let mut adj_info = adj_orig.get();
					if let Some(adj) = turf_gases().get(&loc) {
						if adj_info.last_slow_queue_cycle == queue_cycle_slow
							|| adj.value().planetary_atmos.is_some()
						{
							continue;
						}
						unsafe { Value::turf_by_id_unchecked(i as u32) }.call(
							"consider_firelocks",
							&[&unsafe { Value::turf_by_id_unchecked(loc as u32) }],
						)?;
						let (_, new_m) = progression_order[queue_idx];
						if new_m.adjacency & bit == bit {
							adj_info.last_slow_queue_cycle = queue_cycle_slow;
							adj_info.curr_transfer_dir = OPP_DIR_INDEX[j as usize];
							progression_order.push((*adj.key(), *adj.value()));
							adj_orig.set(adj_info);
						}
					}
				}
			}
		}
		queue_idx += 1;
	}
	for (i, _) in progression_order.iter().rev() {
		let cur_orig = info.get(i).unwrap();
		let mut cur_info = cur_orig.get();
		let airflow = cur_info.mole_delta - target_delta;
		let adj_orig = info
			.get(&adjacent_tile_id(
				cur_info.curr_transfer_dir as u8,
				*i,
				max_x,
				max_y,
			))
			.unwrap();
		let mut adj_info = adj_orig.get();
		cur_info.adjust_eq_movement(&mut adj_info, cur_info.curr_transfer_dir, airflow);
		if cur_info.curr_transfer_dir != 6 {
			adj_info.mole_delta += airflow;
		}
		cur_info.mole_delta = target_delta;
		cur_orig.set(cur_info);
		adj_orig.set(adj_info);
	}
	Ok(Value::null())
}

pub(crate) fn equalize(
	equalize_turf_limit: usize,
	equalize_hard_turf_limit: usize,
	max_x: i32,
	max_y: i32,
	high_pressure_turfs: BTreeSet<TurfID>,
) -> usize {
	let mut info: BTreeMap<TurfID, Cell<MonstermosInfo>> = BTreeMap::new();
	let mut turfs_processed = 0;
	let mut queue_cycle_slow = 1;
	let mut found_turfs: BTreeSet<TurfID> = BTreeSet::new();
	for &i in high_pressure_turfs.iter() {
		if found_turfs.contains(&i)
			|| turf_gases().get(&i).map_or(true, |m| {
				!m.enabled()
					|| m.adjacency <= 0 || GasArena::with_all_mixtures(|all_mixtures| {
					let our_moles = all_mixtures[m.mix].read().total_moles();
					our_moles < 10.0
						|| m.adjacent_mixes(all_mixtures).all(|lock| {
							(lock.read().total_moles() - our_moles).abs()
								< MINIMUM_MOLES_DELTA_TO_MOVE
						})
				})
			}) {
			continue;
		}
		let m = turf_gases().get(&i).unwrap();
		let maybe_turfs = flood_fill_equalize_turfs(
			i,
			*m,
			equalize_turf_limit,
			equalize_hard_turf_limit,
			max_x,
			max_y,
			&mut found_turfs,
			&mut info,
		);
		if maybe_turfs.is_none() {
			continue;
		}
		let (mut turfs, planet_turfs, total_moles) = maybe_turfs.unwrap();
		if turfs.len() > equalize_turf_limit {
			// throw out any above turf limit, we check more for explosive decomp
			for (idx, _) in turfs.drain(equalize_turf_limit..) {
				found_turfs.remove(&idx);
			}
		}
		let average_moles = (total_moles / (turfs.len() - planet_turfs.len()) as f64) as f32;
		let (mut giver_turfs, mut taker_turfs) = partition_turfs(&turfs, &mut info, average_moles);
		let log_n = ((turfs.len() as f32).log2().floor()) as usize;
		if giver_turfs.len() > log_n && taker_turfs.len() > log_n {
			turfs.sort_by_cached_key(|(idx, _)| {
				float_ord::FloatOrd(info.get(idx).unwrap().get().mole_delta)
			});
			for &(i, m) in &turfs {
				monstermos_fast_process(i, m, max_x, max_y, &mut info);
			}
			giver_turfs.clear();
			taker_turfs.clear();
			for &(i,m) in &turfs {
				if info.entry(i).or_default().get().mole_delta > 0.0 {
					giver_turfs.push((i,m));
				} else {
					taker_turfs.push((i,m));
				}
			}
		}
		// alright this is the part that can become O(n^2).
		if giver_turfs.len() < taker_turfs.len() {
			// as an optimization, we choose one of two methods based on which list is smaller.
			give_to_takers(
				&giver_turfs,
				&taker_turfs,
				max_x,
				max_y,
				&info,
				&mut queue_cycle_slow,
			);
		} else {
			take_from_givers(
				&taker_turfs,
				&giver_turfs,
				max_x,
				max_y,
				&info,
				&mut queue_cycle_slow,
			);
		}
		if !planet_turfs.is_empty() {
			turfs_processed += turfs.len() + planet_turfs.len();
			let sender = byond_callback_sender();
			let fake_cloned = info
				.iter()
				.map(|(&k, v)| (k, v.get()))
				.collect::<BTreeMap<TurfID, MonstermosInfo>>();
			let _ = sender.send(Box::new(move || {
				let mut cloned = fake_cloned
					.iter()
					.map(|(&k, &v)| (k, Cell::new(v)))
					.collect::<BTreeMap<TurfID, Cell<MonstermosInfo>>>();
				process_planet_turfs(
					&planet_turfs,
					average_moles,
					max_x,
					max_y,
					&mut cloned,
					queue_cycle_slow,
				)?;
				for (i, turf) in turfs.iter() {
					finalize_eq(*i, turf, &cloned, max_x, max_y);
				}
				Ok(Value::null())
			}));
		} else {
			turfs_processed += turfs.len();
			for (i, turf) in turfs.iter() {
				finalize_eq(*i, turf, &info, max_x, max_y);
			}
		}
	}
	turfs_processed
}
