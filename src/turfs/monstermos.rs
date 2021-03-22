use super::*;

use std::collections::VecDeque;

use std::collections::{BTreeMap, BTreeSet};

use auxcallback::{callback_sender_by_id_insert, process_callbacks_for_millis};

use std::cell::Cell;

use std::sync::atomic::{AtomicU8, Ordering};

type TransferInfo = [f32; 7];

#[derive(Copy, Clone, Default)]
struct MonstermosInfo {
	transfer_dirs: TransferInfo,
	mole_delta: f32,
	curr_transfer_amount: f32,
	curr_transfer_dir: usize,
	last_slow_queue_cycle: i32,
	fast_done: bool,
	last_cycle: i32,
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

lazy_static! {
	static ref MONSTERMOS_TURF_CHANNEL: (
		flume::Sender<BTreeMap<TurfID, (TurfMixture, Cell<TransferInfo>)>>,
		flume::Receiver<BTreeMap<TurfID, (TurfMixture, Cell<TransferInfo>)>>
	) = flume::unbounded();
}

const EQUALIZATION_NONE: u8 = 0;
const EQUALIZATION_PROCESSING: u8 = 1;
const EQUALIZATION_FINALIZING: u8 = 2;
const EQUALIZATION_DONE: u8 = 3;

static EQUALIZATION_STEP: AtomicU8 = AtomicU8::new(0);

fn finalize_eq(
	i: TurfID,
	turf: &TurfMixture,
	monstermos_orig: &Cell<TransferInfo>,
	other_turfs: &BTreeMap<TurfID, (TurfMixture, Cell<TransferInfo>)>,
	max_x: i32,
	max_y: i32,
) {
	let sender = callback_sender_by_id_insert(SSAIR_NAME.to_string());
	let transfer_dirs = monstermos_orig.take();
	let planet_transfer_amount = transfer_dirs[6];
	if planet_transfer_amount > 0.0 {
		if turf.total_moles() < planet_transfer_amount {
			finalize_eq_neighbors(i, &transfer_dirs, turf, other_turfs, max_x, max_y);
		}
		GasMixtures::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(turf.mix)
				.unwrap()
				.write()
				.remove(planet_transfer_amount);
		})
	} else if planet_transfer_amount < 0.0 {
		if let Some(air_entry) = PLANETARY_ATMOS.get(turf.planetary_atmos.unwrap()) {
			let planet_air = air_entry.value();
			let planet_sum = planet_air.total_moles();
			if planet_sum > 0.0 {
				GasMixtures::with_all_mixtures(|all_mixtures| {
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
				finalize_eq_neighbors(i, &transfer_dirs, turf, other_turfs, max_x, max_y);
			}
			if let Some((adj_turf, adj_orig)) = other_turfs.get(&adj_id) {
				let mut adj_info = adj_orig.take();
				adj_info[OPP_DIR_INDEX[j as usize]] = 0.0;
				if turf.mix != adj_turf.mix {
					GasMixtures::with_all_mixtures(|all_mixtures| {
						let our_entry = all_mixtures.get(turf.mix).unwrap();
						let their_entry = all_mixtures.get(adj_turf.mix).unwrap();
						let mut air = our_entry.write();
						let mut other_air = their_entry.write();
						other_air.merge(&air.remove(amount));
					});
				}
				adj_orig.set(adj_info);
				sender
					.send(Box::new(move |_| {
						let real_amount = Value::from(-amount);
						let turf = unsafe { Value::turf_by_id_unchecked(i as u32) };
						let other_turf = unsafe { Value::turf_by_id_unchecked(adj_id as u32) };
						if let Err(e) =
							turf.call("consider_pressure_difference", &[&other_turf, &real_amount])
						{
							turf.call("stack_trace", &[&Value::from_string(e.message.as_str())])
								.unwrap();
						}
						Ok(Value::null())
					}))
					.unwrap();
			}
		}
	}
}

fn finalize_eq_neighbors(
	i: TurfID,
	transfer_dirs: &[f32; 7],
	turf: &TurfMixture,
	other_turfs: &BTreeMap<TurfID, (TurfMixture, Cell<TransferInfo>)>,
	max_x: i32,
	max_y: i32,
) {
	for (j, adjacent_id) in adjacent_tile_ids(turf.adjacency, i, max_x, max_y) {
		let amount = transfer_dirs[j as usize];
		if amount < 0.0 {
			if let Some((other_turf, other_info)) = other_turfs.get(&adjacent_id) {
				finalize_eq(
					adjacent_id,
					other_turf,
					other_info,
					other_turfs,
					max_x,
					max_y,
				);
			}
		}
	}
}

#[cfg(feature = "explosive_decompression")]
fn explosively_depressurize(
	turf_idx: TurfID,
	turf: TurfMixture,
	info: &mut BTreeMap<TurfID, Cell<[MonstermosInfo]>>,
	queue_cycle_slow: &mut i32,
	equalize_hard_turf_limit: usize,
	max_x: i32,
	max_y: i32,
) {
	let mut total_gases_deleted = 0.0;
	let mut turfs: Vec<(TurfID, TurfMixture)> = Vec::new();
	let mut space_turfs: Vec<(TurfID, TurfMixture)> = Vec::new();
	turfs.push((turf_idx, turf));
	let cur_orig = info.get(&turf_idx).unwrap();
	let mut cur_info: MonstermosInfo = Default::default();
	cur_info.done_this_cycle = true;
	cur_info.curr_transfer_dir = 6;
	cur_orig.set(cur_info);
	let mut warned_about_planet_atmos = false;
	let mut cur_queue_idx = 0;
	let sender = callback_sender_by_id_insert(SSAIR_NAME.to_string());
	while cur_queue_idx < turfs.len() {
		let (i, m) = turfs[cur_queue_idx];
		cur_queue_idx += 1;
		let cur_orig = info.get(&i).unwrap();
		let mut cur_info = cur_orig.get();
		cur_info.done_this_cycle = true;
		cur_info.curr_transfer_dir = 6;
		cur_orig.set(cur_info);
		if m.planetary_atmos.is_some() {
			warned_about_planet_atmos = true;
			continue;
		}
		if m.is_immutable() {
			space_turfs.push((i, m));
			sender
				.send(Box::new(move |_| {
					unsafe { Value::turf_by_id_unchecked(i) }.set(
						"pressure_specific_target",
						&[&unsafe { Value::turf_by_id_unchecked(i) }],
					)?;
					Ok(Value::null())
				}))
				.unwrap();
		} else {
			if cur_queue_idx > equalize_hard_turf_limit {
				continue;
			}
			for j in 0..6 {
				let bit = 1 << j;
				if m.adjacency & bit == bit {
					let loc = adjacent_tile_id(j as u8, i, max_x, max_y);
					if let Some(adj) = TURF_GASES.get(&loc) {
						let (&adj_i, &adj_m) = (adj.key(), adj.value());
						let adj_orig = info.entry(loc).or_default();
						let mut adj_info = adj_orig.get();
						if adj_info.done_this_cycle {
							continue;
						}
						let (call_result_sender, call_result_receiver) = flume::bounded(0);
						sender
							.send(Box::new(move |_| {
								unsafe { Value::turf_by_id_unchecked(i) }.call(
									"consider_firelocks",
									&[&unsafe { Value::turf_by_id_unchecked(loc) }],
								)?;
								call_result_sender.send(true).unwrap();
								Ok(Value::null())
							}))
							.unwrap();
						call_result_receiver.recv().unwrap();
						let new_m = TURF_GASES[i];
						if new_m.adjacency & bit == bit {
							adj_info = Default::default();
							adj_info.done_this_cycle = true;
							adj_orig.set(adj_info);
							turfs.push((adj_i, adj_m));
						}
					}
				}
			}
		}
		if warned_about_planet_atmos {
			return; // planet atmos > space
		}
	}
	*queue_cycle_slow += 1;
	let mut progression_order: Vec<(TurfID, TurfMixture)> = Vec::with_capacity(space_turfs.len());
	for (i, m) in space_turfs.iter() {
		progression_order.push((*i, *m));
		let cur_info = info.get_mut(i).unwrap().get_mut();
		cur_info.last_slow_queue_cycle = *queue_cycle_slow;
		cur_info.curr_transfer_dir = 6;
	}
	cur_queue_idx = 0;
	while cur_queue_idx < progression_order.len() {
		let (i, m) = progression_order[cur_queue_idx];
		for (j, loc) in adjacent_tile_ids(m.adjacency, i, max_x, max_y) {
			if let Some(adj) = TURF_GASES.get(&loc) {
				let (adj_i, adj_m) = (adj.key(), adj.value());
				let adj_orig = info.entry(loc).or_default();
				let mut adj_info = adj_orig.get();
				if adj_info.done_this_cycle
					&& adj_info.last_slow_queue_cycle != *queue_cycle_slow
					&& !adj_m.is_immutable()
				{
					adj_info.curr_transfer_dir = OPP_DIR_INDEX[j as usize];
					adj_info.curr_transfer_amount = 0.0;
					sender
						.send(Box::new(move |_| {
							unsafe { Value::turf_by_id_unchecked(adj_i) }.set(
								"pressure_specific_target",
								&[&unsafe { Value::turf_by_id_unchecked(i) }],
							)?;
							Ok(Value::null())
						}))
						.unwrap();
					adj_info.last_slow_queue_cycle = *queue_cycle_slow;
					adj_orig.set(adj_info);
					progression_order.push((*adj_i, *adj_m));
				}
			}
			cur_queue_idx += 1;
		}
	}
	let mut hpd_entries: Vec<ByondArg> = Vec::new();
	for (i, m) in progression_order.iter().rev() {
		let cur_orig = info.get(i).unwrap();
		let mut cur_info = cur_orig.get();
		if cur_info.curr_transfer_dir == 6 {
			continue;
		}
		hpd_entries.push(ByondArg::Turf(*i as u32));
		let loc = adjacent_tile_id(cur_info.curr_transfer_dir as u8, *i, max_x, max_y);
		if let Some(adj) = TURF_GASES.get(&loc) {
			let (adj_i, adj_m) = (adj.key(), adj.value());
			let adj_orig = info.get(&loc).unwrap();
			let mut adj_info = adj_orig.get();
			let sum = adj_m.total_moles();
			total_gases_deleted += sum;
			cur_info.curr_transfer_amount += sum;
			adj_info.curr_transfer_amount += cur_info.curr_transfer_amount;
			if adj_info.curr_transfer_dir == 6 {
				sender
					.send(Box::new(move |_| {
						let byond_turf = unsafe { Value::turf_by_id_unchecked(adj_i) };
						byond_turf.set("pressure_difference", cur_info.curr_transfer_amount)?;
						byond_turf.set(
							"pressure_direction",
							(1 << cur_info.curr_transfer_dir) as f32,
						)?;
						Ok(Value::null())
					}))
					.unwrap();
			}
			m.clear_air();
			sender
				.send(Box::new(move |_| {
					let byond_turf = unsafe { Value::turf_by_id_unchecked(i) };
					byond_turf.set("pressure_difference", cur_info.curr_transfer_amount)?;
					byond_turf.set(
						"pressure_direction",
						(1 << cur_info.curr_transfer_dir) as f32,
					)?;
					byond_turf.call("handle decompression floor rip", &[&Value::from(sum)])?;
					Ok(Value::null())
				}))
				.unwrap();
		}
	}
	if (total_gases_deleted / turfs.len() as f32) > 20.0 && turfs.len() > 10 { // logging I guess
	}
}

// In its own function due to the Rust compiler not liking a massive function in a hook.
fn actual_equalize(src: &Value, args: &[Value], ctx: &DMContext) -> DMResult {
	let equalize_turf_limit = src.get_number("equalize_turf_limit")? as usize;
	let equalize_hard_turf_limit = src.get_number("equalize_hard_turf_limit")? as usize;
	let max_x = ctx.get_world().get_number("maxx")? as i32;
	let max_y = ctx.get_world().get_number("maxy")? as i32;
	let turf_receiver = HIGH_PRESSURE_TURFS.1.clone();
	let resumed = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of arguments to turf equalization: 0"))?
		.as_number()?
		== 1.0;
	if !turf_receiver.is_empty()
		&& EQUALIZATION_STEP.compare_exchange(
			EQUALIZATION_NONE,
			EQUALIZATION_PROCESSING,
			Ordering::SeqCst,
			Ordering::Relaxed,
		) == Ok(EQUALIZATION_NONE)
	{
		rayon::spawn(move || {
			let mut info: BTreeMap<TurfID, Cell<MonstermosInfo>> = BTreeMap::new();
			let mut queue_cycle_slow = 1;
			let mut queue_cycle = 1;
			let turf_sender = MONSTERMOS_TURF_CHANNEL.0.clone();
			let sender = callback_sender_by_id_insert(SSAIR_NAME.to_string());
			for i in turf_receiver.try_iter() {
				if let Some(m) = TURF_GASES.get(&i) {
					if m.simulation_level == SIMULATION_LEVEL_ALL && m.adjacency > 0 {
						let our_info = info.entry(i).or_default().get();
						if our_info.last_cycle >= queue_cycle {
							continue;
						}
						let our_moles = m.total_moles();
						if our_moles < 10.0 {
							continue;
						}
						let adj_tiles = adjacent_tile_ids(m.adjacency, i, max_x, max_y);
						let mut any_comparison_good = false;
						for (_, loc) in adj_tiles.iter() {
							if let Some(gas) = TURF_GASES.get(loc) {
								if (gas.total_moles() - our_moles).abs()
									> MINIMUM_MOLES_DELTA_TO_MOVE
								{
									any_comparison_good = true;
									break;
								}
							}
						}
						if !any_comparison_good {
							continue;
						}
					} else {
						continue;
					}
					queue_cycle += 1;
					let mut turfs: Vec<(TurfID, TurfMixture)> =
						Vec::with_capacity(equalize_turf_limit);
					let mut border_turfs: VecDeque<(TurfID, TurfMixture)> =
						VecDeque::with_capacity(equalize_turf_limit);
					let mut found_turfs: BTreeSet<TurfID> = BTreeSet::new();
					let mut planet_turfs: Vec<(TurfID, TurfMixture)> = Vec::new();
					let mut total_moles: f32 = 0.0;
					#[allow(unused_mut)]
					let mut space_this_time = false;
					#[warn(unused_mut)]
					border_turfs.push_back((i, *m));
					info.entry(i).or_default().get_mut().last_cycle = queue_cycle;
					while border_turfs.len() > 0 {
						if turfs.len() > equalize_hard_turf_limit {
							break;
						}
						let (cur_idx, cur_turf) = border_turfs.pop_front().unwrap();
						if turfs.len() < equalize_turf_limit {
							if cur_turf.planetary_atmos.is_some() {
								planet_turfs.push((cur_idx, cur_turf));
								continue;
							}
							total_moles += cur_turf.total_moles();
						}
						for (_, loc) in
							adjacent_tile_ids(cur_turf.adjacency, cur_idx, max_x, max_y).iter()
						{
							if found_turfs.contains(loc) {
								continue;
							}
							if let Some(adj_turf) = TURF_GASES.get(loc) {
								let adj_orig = info.entry(cur_idx).or_default();
								let mut adj_info = adj_orig.get();
								#[cfg(feature = "explosive_decompression")]
								{
									if !adj_info.last_cycle != queue_cycle {
										adj_info = Default::default();
										adj_info.last_cycle = queue_cycle;
										adj_orig.set(adj_info);
										border_turfs.push_back((*loc, *adj_turf.value()));
										if adj_turf.value().is_immutable() {
											// Uh oh! looks like someone opened an airlock to space! TIME TO SUCK ALL THE AIR OUT!!!
											// NOT ONE OF YOU IS GONNA SURVIVE THIS
											// (I just made explosions less laggy, you're welcome)
											turfs.push((*loc, *adj_turf.value()));
											explosively_depressurize(
												*i,
												*m,
												&mut info,
												&mut queue_cycle_slow,
												equalize_hard_turf_limit,
											);
											space_this_time = true;
										}
									}
								}
								#[cfg(not(feature = "explosive_decompression"))]
								{
									if !adj_info.last_cycle != queue_cycle
										&& adj_turf.simulation_level != SIMULATION_LEVEL_NONE
										&& (adj_turf.simulation_level & SIMULATION_LEVEL_DISABLED
											!= SIMULATION_LEVEL_DISABLED)
									{
										adj_info = Default::default();
										adj_info.last_cycle = queue_cycle;
										adj_orig.set(adj_info);
										border_turfs.push_back((*loc, *adj_turf.value()));
									}
								}
							}
							if space_this_time {
								break;
							}
							found_turfs.insert(cur_idx);
							turfs.push((cur_idx, cur_turf));
						}
					}
					if space_this_time {
						continue;
					}
					if turfs.len() > equalize_turf_limit {
						for i in equalize_turf_limit..turfs.len() {
							let (idx, _) = turfs[i];
							info.entry(idx).or_default().get_mut().last_cycle = 0;
						}
						turfs.resize(equalize_turf_limit, Default::default());
					}
					let average_moles =
						total_moles / (turfs.len() as f32 - planet_turfs.len() as f32);
					let mut giver_turfs: Vec<(TurfID, TurfMixture)> =
						Vec::with_capacity(turfs.len() / 2);
					let mut taker_turfs: Vec<(TurfID, TurfMixture)> =
						Vec::with_capacity(turfs.len() / 2);
					for (i, m) in turfs.iter() {
						let cur_info = info.entry(*i).or_default().get_mut();
						cur_info.last_cycle = queue_cycle;
						cur_info.mole_delta = m.total_moles() - average_moles;
						if cur_info.mole_delta > 0.0 {
							giver_turfs.push((*i, *m));
						} else {
							taker_turfs.push((*i, *m));
						}
					}
					let log_n = ((turfs.len() as f32).log2().floor()) as usize;
					if giver_turfs.len() > log_n && taker_turfs.len() > log_n {
						turfs.sort_by_cached_key(|(idx, _)| {
							float_ord::FloatOrd(info.get(idx).unwrap().get().mole_delta)
						});
						for (i, m) in turfs.iter() {
							let cur_orig = info.get(i).unwrap();
							let mut cur_info = cur_orig.take();
							cur_info.fast_done = true;
							let mut eligible_adjacents: i32 = 0;
							if cur_info.mole_delta > 0.0 {
								for (j, loc) in adjacent_tile_ids(m.adjacency, *i, max_x, max_y) {
									if let Some(adj_orig) = info.get(&loc) {
										let adj_info = adj_orig.get();
										if adj_info.fast_done || adj_info.last_cycle != queue_cycle
										{
											continue;
										} else {
											eligible_adjacents |= 1 << j;
										}
									}
								}
								let amt_eligible = eligible_adjacents.count_ones();
								if amt_eligible == 0 {
									continue;
								}
								let moles_to_move = cur_info.mole_delta / amt_eligible as f32;
								for j in 0..6 {
									let bit = 1 << j;
									if eligible_adjacents & bit == bit {
										let adj_orig = info
											.get(&adjacent_tile_id(j, *i, max_x, max_y))
											.unwrap();
										let mut adj_info = adj_orig.take();
										cur_info.adjust_eq_movement(
											&mut adj_info,
											j as usize,
											moles_to_move,
										);
										cur_info.mole_delta -= moles_to_move;
										adj_info.mole_delta += moles_to_move;
										adj_orig.set(adj_info);
									}
								}
							}
							cur_orig.set(cur_info);
						}
						giver_turfs.clear();
						taker_turfs.clear();
						for (i, m) in turfs.iter() {
							let cur_orig = info.get(i).unwrap();
							let cur_info = cur_orig.get();
							if cur_info.mole_delta > 0.0 {
								giver_turfs.push((*i, *m));
							} else {
								taker_turfs.push((*i, *m));
							}
						}
					}
					// alright this is the part that can become O(n^2).
					if giver_turfs.len() < taker_turfs.len() {
						// as an optimization, we choose one of two methods based on which list is smaller. We really want to avoid O(n^2) if we can.
						let mut queue: VecDeque<(TurfID, TurfMixture)> =
							VecDeque::with_capacity(taker_turfs.len());
						for (i, m) in giver_turfs.iter() {
							let giver_orig = info.get(i).unwrap();
							let mut giver_info = giver_orig.get();
							giver_info.curr_transfer_dir = 6;
							giver_info.curr_transfer_amount = 0.0;
							queue_cycle_slow += 1;
							queue.clear();
							queue.push_back((*i, *m));
							giver_info.last_slow_queue_cycle = queue_cycle_slow;
							let mut queue_idx = 0;
							while queue_idx < queue.len() {
								if giver_info.mole_delta <= 0.0 {
									break;
								}
								let (idx, turf) = *queue.get(queue_idx).unwrap();
								for (j, loc) in adjacent_tile_ids(turf.adjacency, idx, max_x, max_y)
								{
									if let Some(adj_orig) = info.get(&loc) {
										if let Some(adj_mix) = TURF_GASES.get(&loc) {
											if giver_info.mole_delta <= 0.0 {
												break;
											}
											let mut adj_info = adj_orig.get();
											if !adj_info.last_cycle == queue_cycle
												&& adj_info.last_slow_queue_cycle
													!= queue_cycle_slow
											{
												queue.push_back((loc, *adj_mix.value()));
												adj_info.last_slow_queue_cycle = queue_cycle_slow;
												adj_info.curr_transfer_dir =
													OPP_DIR_INDEX[j as usize];
												adj_info.curr_transfer_amount = 0.0;
												if adj_info.mole_delta < 0.0 {
													if -adj_info.mole_delta > giver_info.mole_delta
													{
														adj_info.curr_transfer_amount -=
															giver_info.mole_delta;
														adj_info.mole_delta +=
															giver_info.mole_delta;
														giver_info.mole_delta = 0.0;
													} else {
														adj_info.curr_transfer_amount +=
															adj_info.mole_delta;
														giver_info.mole_delta +=
															adj_info.mole_delta;
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
								if turf_info.curr_transfer_amount != 0.0
									&& turf_info.curr_transfer_dir != 6
								{
									let adj_tile_id = adjacent_tile_id(
										turf_info.curr_transfer_dir as u8,
										idx,
										max_x,
										max_y,
									);
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
						}
					} else {
						let mut queue: VecDeque<(TurfID, TurfMixture)> =
							VecDeque::with_capacity(giver_turfs.len());
						for (i, m) in taker_turfs.iter() {
							let taker_orig = info.get(i).unwrap();
							let mut taker_info = taker_orig.get();
							taker_info.curr_transfer_dir = 6;
							taker_info.curr_transfer_amount = 0.0;
							queue_cycle_slow += 1;
							queue.clear();
							queue.push_front((*i, *m));
							taker_info.last_slow_queue_cycle = queue_cycle_slow;
							let mut queue_idx = 0;
							while queue_idx < queue.len() {
								if taker_info.mole_delta >= 0.0 {
									break;
								}
								let (idx, turf) = *queue.get(queue_idx).unwrap();
								for (j, loc) in adjacent_tile_ids(turf.adjacency, idx, max_x, max_y)
								{
									if let Some(adj_orig) = info.get(&loc) {
										if let Some(adj_mix) = TURF_GASES.get(&loc) {
											let mut adj_info = adj_orig.get();
											if taker_info.mole_delta >= 0.0 {
												break;
											}
											if !adj_info.last_cycle == queue_cycle
												&& adj_info.last_slow_queue_cycle
													!= queue_cycle_slow
											{
												queue.push_back((loc, *adj_mix));
												adj_info.last_slow_queue_cycle = queue_cycle_slow;
												adj_info.curr_transfer_dir =
													OPP_DIR_INDEX[j as usize];
												adj_info.curr_transfer_amount = 0.0;
												if adj_info.mole_delta > 0.0 {
													if adj_info.mole_delta > -taker_info.mole_delta
													{
														adj_info.curr_transfer_amount -=
															taker_info.mole_delta;
														adj_info.mole_delta +=
															taker_info.mole_delta;
														taker_info.mole_delta = 0.0;
													} else {
														adj_info.curr_transfer_amount +=
															adj_info.mole_delta;
														taker_info.mole_delta +=
															adj_info.mole_delta;
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
								if turf_info.curr_transfer_amount != 0.0
									&& turf_info.curr_transfer_dir != 6
								{
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
					if !planet_turfs.is_empty() {
						let (_, sample_turf) = planet_turfs[0];
						let planet_sum = PLANETARY_ATMOS
							.get(sample_turf.planetary_atmos.unwrap())
							.unwrap()
							.value()
							.total_moles();
						let target_delta = planet_sum - average_moles;
						queue_cycle_slow += 1;
						let mut progression_order: Vec<(TurfID, TurfMixture)> =
							Vec::with_capacity(planet_turfs.len());
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
										if let Some(adj) = TURF_GASES.get(&loc) {
											if adj_info.last_slow_queue_cycle == queue_cycle_slow
												|| adj.value().planetary_atmos.is_some() || adj_info
												.last_cycle
												!= queue_cycle
											{
												continue;
											}
											let (call_result_sender, call_result_receiver) =
												flume::bounded(0);
											sender
												.send(Box::new(move |_| {
													unsafe {
														Value::turf_by_id_unchecked(i as u32)
													}
													.call(
														"consider_firelocks",
														&[&unsafe {
															Value::turf_by_id_unchecked(loc as u32)
														}],
													)?;
													call_result_sender.send(true).unwrap();
													Ok(Value::null())
												}))
												.unwrap();
											call_result_receiver.recv().unwrap();
											let (_, new_m) = progression_order[queue_idx];
											if new_m.adjacency & bit == bit {
												adj_info.last_slow_queue_cycle = queue_cycle_slow;
												adj_info.curr_transfer_dir =
													OPP_DIR_INDEX[j as usize];
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
							cur_info.adjust_eq_movement(
								&mut adj_info,
								cur_info.curr_transfer_dir,
								airflow,
							);
							if cur_info.curr_transfer_dir != 6 {
								adj_info.mole_delta += airflow;
							}
							cur_info.mole_delta = target_delta;
							cur_orig.set(cur_info);
							adj_orig.set(adj_info);
						}
					}
					let info_to_send = turfs
						.iter()
						.map(|(i, m)| {
							(
								*i,
								(*m, Cell::new(info.get(i).unwrap().get().transfer_dirs)),
							)
						})
						.collect::<BTreeMap<TurfID, (TurfMixture, Cell<TransferInfo>)>>();
					turf_sender.send(info_to_send).unwrap();
				}
			}
			EQUALIZATION_STEP.store(EQUALIZATION_FINALIZING, Ordering::Relaxed);
		});
		rayon::spawn(move || {
			let turf_receiver = MONSTERMOS_TURF_CHANNEL.1.clone();
			while !turf_receiver.is_empty()
				|| EQUALIZATION_STEP.load(Ordering::SeqCst) < EQUALIZATION_FINALIZING
			{
				let mut res = turf_receiver.recv_timeout(std::time::Duration::from_millis(1));
				while res.is_ok() {
					let turf_set = res.unwrap();
					for (i, (turf, monstermos_info)) in turf_set.iter() {
						finalize_eq(*i, turf, monstermos_info, &turf_set, max_x, max_y);
					}
					drop(turf_set);
					res = turf_receiver.recv_timeout(std::time::Duration::from_micros(50));
				}
			}
			EQUALIZATION_STEP.store(EQUALIZATION_DONE, Ordering::SeqCst);
		});
	}
	let arg_limit = args
		.get(1)
		.ok_or_else(|| runtime!("Wrong number of arguments to turf equalization: 1"))?
		.as_number()?;
	if arg_limit <= 0.0 {
		return Ok(Value::from(1.0));
	}
	process_callbacks_for_millis(ctx, SSAIR_NAME.to_string(), arg_limit as u64);
	if let Err(prev_value) = EQUALIZATION_STEP.compare_exchange(
		EQUALIZATION_DONE,
		EQUALIZATION_NONE,
		Ordering::SeqCst,
		Ordering::Relaxed,
	) {
		Ok(Value::from(
			prev_value != EQUALIZATION_DONE && prev_value != EQUALIZATION_NONE,
		))
	} else {
		Ok(Value::from(false))
	}
}

// Expected function call: process_turf_equalize_extools((Master.current_ticklimit - TICK_USAGE) * world.tick_lag)
// Returns: TRUE if not done, FALSE if done
#[hook("/datum/controller/subsystem/air/proc/process_turf_equalize_auxtools")]
fn _hook_equalize() {
	actual_equalize(src, args, ctx)
}
