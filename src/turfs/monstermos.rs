use super::*;

use std::collections::VecDeque;

use std::collections::{BTreeMap, BTreeSet};

use std::cell::Cell;

#[derive(Copy, Clone, Default)]
struct MonstermosInfo {
	transfer_dirs: [f32; 7],
	mole_delta: f32,
	curr_transfer_amount: f32,
	distance_score: f32,
	curr_transfer_dir: usize,
	last_slow_queue_cycle: i32,
	fast_done: bool,
	done_this_cycle: bool,
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

#[allow(dead_code)]
enum ByondArg {
	Turf(u32),
	Float(f32),
	Str(String),
	Null,
}

impl ByondArg {
	pub fn to_usable_arg(&self) -> Value {
		match self {
			Self::Turf(n) => TurfGrid::turf_by_id(*n),
			Self::Float(n) => Value::from(*n),
			Self::Str(s) => Value::from_string(s),
			Self::Null => Value::null(),
		}
	}
	pub fn to_string(&self) -> Option<&str> {
		match self {
			Self::Str(s) => Some(s),
			_ => None,
		}
	}
}

const BLOCKS_CALLER: u32 = 1;

// turf, flags (see above), what to call, arguments
#[cfg(feature = "explosive_decompression")]
type ByondMessage<'a> = (u32, u32, &'a str, Vec<ByondArg>, Option<flume::Sender<bool>>);

lazy_static! {
	static ref EQUALIZE_FINALIZE_CHANNEL: (
		flume::Sender<(usize,usize,f32)>,
		flume::Receiver<(usize,usize,f32)>
	) = flume::unbounded();
	static ref CALL_CHANNEL: (
		flume::Sender<ByondMessage<'static>>,
		flume::Receiver<ByondMessage<'static>>
	) = flume::unbounded();
	static ref MONSTERMOS_TURF_CHANNEL: (
		flume::Sender<BTreeMap<usize, (TurfMixture, Cell<MonstermosInfo>)>>,
		flume::Receiver<BTreeMap<usize, (TurfMixture, Cell<MonstermosInfo>)>>
	) = flume::unbounded();
	static ref HIGH_PRESSURE_DELTA_CHANNEL: (
		flume::Sender<Vec<ByondArg>>,
		flume::Receiver<Vec<ByondArg>>,
	) = flume::unbounded();
}

use std::sync::atomic::{AtomicU8,Ordering};

const EQUALIZATION_NONE: u8 = 0;
const EQUALIZATION_PROCESSING: u8 = 1;
const EQUALIZATION_FINALIZING: u8 = 2;
const EQUALIZATION_DONE: u8 = 3;

static EQUALIZATION_STEP: AtomicU8 = AtomicU8::new(0);

fn finalize_eq(
	i: usize,
	turf: &TurfMixture,
	monstermos_orig: &Cell<MonstermosInfo>,
	other_turfs: &BTreeMap<usize, (TurfMixture, Cell<MonstermosInfo>)>,
	max_x: i32,
	max_y: i32,
) {
	let sender = EQUALIZE_FINALIZE_CHANNEL.0.clone();
	let mut monstermos_info = monstermos_orig.take();
	let transfer_dirs = monstermos_info.transfer_dirs;
	monstermos_info
		.transfer_dirs
		.iter_mut()
		.for_each(|m| *m = 0.0);
	monstermos_orig.set(monstermos_info);
	let planet_transfer_amount = transfer_dirs[6];
	if planet_transfer_amount > 0.0 {
		if turf.total_moles() < planet_transfer_amount {
			finalize_eq_neighbors(i, &transfer_dirs, turf, other_turfs, max_x, max_y);
			monstermos_info = monstermos_orig.get();
		}
		GasMixtures::with_all_mixtures(|all_mixtures| {
			all_mixtures
				.get(turf.mix)
				.unwrap()
				.write()
				.unwrap()
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
						.unwrap()
						.merge(&(planet_air * (-planet_transfer_amount / planet_sum)));
				});
			}
		}
	}
	for j in 0..6 {
		let bit = 1 << j;
		if turf.adjacency & bit == bit {
			let amount = transfer_dirs[j as usize];
			if amount > 0.0 {
				if turf.total_moles() < amount {
					finalize_eq_neighbors(
						i,
						&transfer_dirs,
						turf,
						other_turfs,
						max_x,
						max_y,
					);
					monstermos_info = monstermos_orig.get();
				}
				let adj_id = adjacent_tile_id(j as u8, i, max_x, max_y);
				if let Some((adj_turf, adj_orig)) = other_turfs.get(&adj_id) {
					let mut adj_info = adj_orig.take();
					adj_info.transfer_dirs[OPP_DIR_INDEX[j as usize]] = 0.0;
					if turf.mix != adj_turf.mix {
						GasMixtures::with_all_mixtures(|all_mixtures| {
							let our_entry = all_mixtures.get(turf.mix).unwrap();
							let their_entry = all_mixtures.get(adj_turf.mix).unwrap();
							let mut air = our_entry.write().unwrap();
							let mut other_air = their_entry.write().unwrap();
							other_air.merge(&air.remove(amount));
						});
					}
					adj_orig.set(adj_info);
					monstermos_orig.set(monstermos_info);
					sender.send((i, adj_id, amount)).unwrap();
				}
			}
		}
	}
}

fn finalize_eq_neighbors(
	i: usize,
	transfer_dirs: &[f32; 7],
	turf: &TurfMixture,
	other_turfs: &BTreeMap<usize, (TurfMixture, Cell<MonstermosInfo>)>,
	max_x: i32,
	max_y: i32,
) {
	for j in 0..6 {
		let amount = transfer_dirs[j];
		let bit = 1 << j;
		if amount < 0.0 && turf.adjacency & bit == bit {
			let adjacent_id = adjacent_tile_id(j as u8, i, max_x, max_y);
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
	turf_idx: usize,
	turf: TurfMixture,
	info: &mut BTreeMap<usize, Cell<MonstermosInfo>>,
	queue_cycle_slow: &mut i32,
	monstermos_hard_turf_limit: usize,
) {
	let mut total_gases_deleted = 0.0;
	let mut turfs: Vec<(usize, TurfMixture)> = Vec::new();
	let mut space_turfs: Vec<(usize, TurfMixture)> = Vec::new();
	turfs.push((turf_idx, turf));
	let max_x = TurfGrid::max_x();
	let max_y = TurfGrid::max_y();
	let cur_orig = info.get(&turf_idx).unwrap();
	let mut cur_info: MonstermosInfo = Default::default();
	cur_info.done_this_cycle = true;
	cur_info.curr_transfer_dir = 6;
	cur_orig.set(cur_info);
	let mut warned_about_planet_atmos = false;
	let mut cur_queue_idx = 0;
	let call_sender = CALL_CHANNEL.0.clone();
	let hpd_sender = HIGH_PRESSURE_DELTA_CHANNEL.0.clone();
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
			call_sender
				.send((
					i as u32,
					0,
					"set",
					vec![
						ByondArg::Str("pressure_specific_target".to_string()),
						ByondArg::Turf(i as u32),
					],
					None,
				))
				.unwrap();
		} else {
			if cur_queue_idx > monstermos_hard_turf_limit {
				continue;
			}
			for j in 0..6 {
				let bit = 1 << j;
				if m.adjacency & bit == bit {
					let loc = adjacent_tile_id(j as u8, i, max_x, max_y);
					let adj = TURF_GASES.get(&loc).unwrap();
					let (&adj_i, &adj_m) = (adj.key(),adj.value());
					let adj_orig = info.entry(loc).or_insert(Default::default());
					let mut adj_info = adj_orig.get();
					if adj_info.done_this_cycle {
						continue;
					}
					let (call_result_sender,call_result_receiver) = flume::bounded(0);
					call_sender
						.send((
							i as u32,
							BLOCKS_CALLER,
							"consider_firelocks",
							vec![ByondArg::Turf(loc as u32)],
							Some(call_result_sender),
						))
						.unwrap();
					call_result_receiver.recv().unwrap();
					if m.adjacency & bit == bit {
						adj_info = Default::default();
						adj_info.done_this_cycle = true;
						adj_orig.set(adj_info);
						turfs.push((adj_i, adj_m));
					}
				}
			}
		}
		if warned_about_planet_atmos {
			return; // planet atmos > space
		}
	}
	*queue_cycle_slow += 1;
	let mut progression_order: Vec<(usize, TurfMixture)> = Vec::with_capacity(space_turfs.len());
	for (i, m) in space_turfs.iter() {
		progression_order.push((*i, *m));
		let cur_info = info.get_mut(i).unwrap().get_mut();
		cur_info.last_slow_queue_cycle = *queue_cycle_slow;
		cur_info.curr_transfer_dir = 6;
	}
	cur_queue_idx = 0;
	while cur_queue_idx < progression_order.len() {
		let (i, m) = progression_order[cur_queue_idx];
		for j in 0..6 {
			let bit = 1 << j;
			if m.adjacency & bit == bit {
				let loc = adjacent_tile_id(j as u8, i, max_x, max_y);
				let adj = TURF_GASES.get(&loc).unwrap();
				let (adj_i, adj_m) = (adj.key(), adj.value());
				let adj_orig = info.entry(loc).or_insert(Default::default());
				let mut adj_info = adj_orig.get();
				if adj_info.done_this_cycle
					&& adj_info.last_slow_queue_cycle != *queue_cycle_slow
					&& !adj_m.is_immutable()
				{
					adj_info.curr_transfer_dir = OPP_DIR_INDEX[j as usize];
					adj_info.curr_transfer_amount = 0.0;
					call_sender
						.send((
							*adj_i as u32,
							0,
							"set",
							vec![
								ByondArg::Str("pressure_specific_target".to_string()),
								ByondArg::Turf(i as u32),
							],
							None
						))
						.unwrap();
					adj_info.last_slow_queue_cycle = *queue_cycle_slow;
					adj_orig.set(adj_info);
					progression_order.push((*adj_i, *adj_m));
				}
			}
		}
		cur_queue_idx += 1;
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
		let adj = TURF_GASES.get(&loc).unwrap();
		let (adj_i, adj_m) = (adj.key(), adj.value());
		let adj_orig = info.get(&loc).unwrap();
		let mut adj_info = adj_orig.get();
		let sum = adj_m.total_moles();
		total_gases_deleted += sum;
		cur_info.curr_transfer_amount += sum;
		adj_info.curr_transfer_amount += cur_info.curr_transfer_amount;
		call_sender
			.send((
				*i as u32,
				0,
				"set",
				vec![
					ByondArg::Str("pressure_difference".to_string()),
					ByondArg::Float(cur_info.curr_transfer_amount),
				],
				None,
			))
			.unwrap();
		call_sender
			.send((
				*i as u32,
				0,
				"set",
				vec![
					ByondArg::Str("pressure_direction".to_string()),
					ByondArg::Float(cur_info.curr_transfer_dir as f32),
				],
				None,
			))
			.unwrap();
		if adj_info.curr_transfer_dir == 6 {
			call_sender
				.send((
					*adj_i as u32,
					0,
					"set",
					vec![
						ByondArg::Str("pressure_difference".to_string()),
						ByondArg::Float(cur_info.curr_transfer_amount),
					],
					None,
				))
				.unwrap();
			call_sender
				.send((
					*adj_i as u32,
					0,
					"set",
					vec![
						ByondArg::Str("pressure_direction".to_string()),
						ByondArg::Float(cur_info.curr_transfer_dir as f32),
					],
					None,
				))
				.unwrap();
		}
		m.clear_air();
		call_sender
			.send((*i as u32, 0, "update_visuals", vec![ByondArg::Null],None))
			.unwrap();
		call_sender
			.send((
				*i as u32,
				0,
				"handle decompression floor rip",
				vec![ByondArg::Float(sum)],
				None,
			))
			.unwrap();
	}
	if (total_gases_deleted / turfs.len() as f32) > 20.0 && turfs.len() > 10 { // logging I guess
	}
	hpd_sender.send(hpd_entries).unwrap();
}

fn actual_equalize(src: &Value, args: &[Value]) -> DMResult {
	let monstermos_turf_limit = src.get_number("monstermos_turf_limit")? as usize;
	let monstermos_hard_turf_limit = src.get_number("monstermos_hard_turf_limit")? as usize;
	let time_limit = Duration::from_millis(
		args.get(0)
			.ok_or_else(|| runtime!("Wrong number of arguments to process_turfs_extools: 0"))?
			.as_number()? as u64,
	);
	let start_time = Instant::now();
	let max_x = TurfGrid::max_x();
	let max_y = TurfGrid::max_y();
	rayon::spawn(move || {
		let mut info: BTreeMap<usize, Cell<MonstermosInfo>> = BTreeMap::new();
		let mut queue_cycle_slow = 0;
		let call_sender = CALL_CHANNEL.0.clone();
		let turf_sender = MONSTERMOS_TURF_CHANNEL.0.clone();
		EQUALIZATION_STEP.store(EQUALIZATION_PROCESSING,Ordering::Relaxed);
		for e in TURF_GASES.iter() {
			let (i, m) = (e.key(), e.value());
			if m.simulation_level >= SIMULATION_LEVEL_SIMULATE && m.adjacency > 0 {
				if let Some(our_info) = info.get(i) {
					if our_info.get().done_this_cycle {
						continue;
					}
				}
				let adj_tiles = adjacent_tile_ids(m.adjacency, *i, max_x, max_y);
				let mut any_comparison_good = false;
				let our_moles = m.total_moles();
				for loc in adj_tiles.iter() {
					if let Some(gas) = TURF_GASES.get(loc) {
						if (gas.total_moles() - our_moles).abs() > MINIMUM_MOLES_DELTA_TO_MOVE {
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
			let mut turfs: Vec<(usize, TurfMixture)> = Vec::with_capacity(monstermos_turf_limit);
			let mut border_turfs: VecDeque<(usize, TurfMixture)> =
				VecDeque::with_capacity(monstermos_turf_limit);
			let mut found_turfs: BTreeSet<usize> = BTreeSet::new();
			let mut planet_turfs: Vec<(usize, TurfMixture)> = Vec::new();
			let mut total_moles: f32 = 0.0;
			#[allow(unused_mut)]
			let mut space_this_time = false;
			#[warn(unused_mut)]
			border_turfs.push_back((*i, *m));
			while border_turfs.len() > 0 {
				if turfs.len() > monstermos_hard_turf_limit {
					break;
				}
				let (cur_idx, cur_turf) = border_turfs.pop_front().unwrap();
				let cur_orig = info.entry(cur_idx).or_insert(Default::default());
				let mut cur_info = cur_orig.get();
				cur_info.distance_score = 0.0;
				if turfs.len() < monstermos_turf_limit {
					let turf_moles = cur_turf.total_moles();
					cur_info.mole_delta = turf_moles;
					cur_orig.set(cur_info);
					if cur_turf.planetary_atmos.is_some() {
						planet_turfs.push((cur_idx, cur_turf));
						continue;
					}
					total_moles += turf_moles;
				}
				for loc in adjacent_tile_ids(cur_turf.adjacency, cur_idx, max_x, max_y).iter() {
					if found_turfs.contains(loc) {
						continue;
					}
					let adj_turf = TURF_GASES.get(loc).unwrap();
					let adj_orig = info.entry(cur_idx).or_insert(Default::default());
					let mut adj_info = adj_orig.get();
					#[cfg(feature = "explosive_decompression")]
					{
						if !adj_info.done_this_cycle {
							adj_info.done_this_cycle = true;
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
									monstermos_hard_turf_limit,
								);
								space_this_time = true;
							}
						}
					}
					#[cfg(not(feature = "explosive_decompression"))]
					{
						if !adj_info.done_this_cycle
							&& adj_turf.simulation_level != SIMULATION_LEVEL_NONE
						{
							adj_info.done_this_cycle = true;
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
			if space_this_time {
				continue;
			}
			if turfs.len() > monstermos_turf_limit {
				for i in monstermos_turf_limit..turfs.len() {
					let (idx, _) = turfs[i];
					info.entry(idx)
						.or_insert(Default::default())
						.get_mut()
						.done_this_cycle = false;
				}
				turfs.resize(monstermos_turf_limit,Default::default());
			}
			let average_moles = total_moles / (turfs.len() as f32);
			let mut giver_turfs: Vec<(usize, TurfMixture)> = Vec::new();
			let mut taker_turfs: Vec<(usize, TurfMixture)> = Vec::new();
			for (i, m) in turfs.iter() {
				let cur_info = info.entry(*i).or_insert(Default::default()).get_mut();
				cur_info.mole_delta -= average_moles;
				if cur_info.mole_delta > 0.0 {
					giver_turfs.push((*i, *m));
				} else {
					taker_turfs.push((*i, *m));
				}
			}
			let log_n = ((turfs.len() as f32).log2().floor()) as usize;
			if giver_turfs.len() > log_n && taker_turfs.len() > log_n {
				use float_ord::FloatOrd;
				turfs.sort_by_cached_key(|(idx, _)| {
					FloatOrd(info.get(idx).unwrap().get().mole_delta)
				});
				for (i, m) in turfs.iter() {
					let cur_orig = info.get(i).unwrap();
					let mut cur_info = cur_orig.get();
					cur_info.fast_done = true;
					let mut eligible_adjacents = 0;
					if cur_info.mole_delta > 0.0 {
						for j in 0..6 {
							let bit = 1 << j;
							if m.adjacency & bit == bit {
								let loc = adjacent_tile_id(j, *i, max_x, max_y);
								if let Some(adj_orig) = info.get(&loc) {
									let adj_info = adj_orig.get();
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
								let adj_orig =
									info.get(&adjacent_tile_id(j, *i, max_x, max_y)).unwrap();
								let mut adj_turf = adj_orig.get();
								cur_info.adjust_eq_movement(
									&mut adj_turf,
									j as usize,
									moles_to_move,
								);
								cur_info.mole_delta -= moles_to_move;
								adj_turf.mole_delta += moles_to_move;
								adj_orig.set(adj_turf);
								cur_orig.set(cur_info);
							}
						}
					}
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
					cur_orig.set(cur_info);
				}
			}
			// alright this is the part that can become O(n^2).
			if giver_turfs.len() < taker_turfs.len() {
				// as an optimization, we choose one of two methods based on which list is smaller. We really want to avoid O(n^2) if we can.
				let mut queue: VecDeque<(usize, TurfMixture)> =
					VecDeque::with_capacity(taker_turfs.len());
				for (i, m) in giver_turfs.iter() {
					let giver_orig = info.get(i).unwrap();
					let mut giver_info = giver_orig.get();
					giver_info.curr_transfer_dir = 6;
					giver_info.curr_transfer_amount = 0.0;
					queue_cycle_slow += 1;
					queue.clear();
					queue.push_front((*i, *m));
					let mut queue_idx = 0;
					while queue_idx < queue.len() {
						if giver_info.mole_delta <= 0.0 {
							break;
						}
						let (idx, turf) = *queue.get(queue_idx).unwrap();
						for j in 0..6 {
							let bit = 1 << j;
							if turf.adjacency & bit == bit {
								if let Some(adj_orig) =
									info.get(&adjacent_tile_id(j, idx, max_x, max_y))
								{
									if giver_info.mole_delta <= 0.0 {
										break;
									}
									let mut adj_info = adj_orig.get();
									if !adj_info.done_this_cycle
										&& adj_info.last_slow_queue_cycle != queue_cycle_slow
									{
										queue.push_back((*i, *TURF_GASES.get(i).unwrap().value()));
										adj_info.last_slow_queue_cycle = queue_cycle_slow;
										adj_info.curr_transfer_dir = OPP_DIR_INDEX[j as usize];
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
						if turf_info.curr_transfer_amount > 0.0 && turf_info.curr_transfer_dir != 6
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
				let mut queue: VecDeque<(usize, TurfMixture)> =
					VecDeque::with_capacity(giver_turfs.len());
				for (i, m) in taker_turfs.iter() {
					let taker_orig = info.get(i).unwrap();
					let mut taker_info = taker_orig.get();
					taker_info.curr_transfer_dir = 6;
					taker_info.curr_transfer_amount = 0.0;
					queue_cycle_slow += 1;
					queue.clear();
					queue.push_front((*i, *m));
					let mut queue_idx = 0;
					while queue_idx < queue.len() {
						if taker_info.mole_delta >= 0.0 {
							break;
						}
						let (idx, turf) = *queue.get(queue_idx).unwrap();
						for j in 0..6 {
							let bit = 1 << j;
							if turf.adjacency & bit == bit {
								if let Some(adj_orig) =
									info.get(&adjacent_tile_id(j, idx, max_x, max_y))
								{
									let mut adj_info = adj_orig.get();
									if taker_info.mole_delta >= 0.0 {
										break;
									}
									if !adj_info.done_this_cycle
										&& adj_info.last_slow_queue_cycle != queue_cycle_slow
									{
										queue.push_back((*i, *TURF_GASES.get(i).unwrap().value()));
										adj_info.last_slow_queue_cycle = queue_cycle_slow;
										adj_info.curr_transfer_dir = OPP_DIR_INDEX[j as usize];
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
						if turf_info.curr_transfer_amount > 0.0 && turf_info.curr_transfer_dir != 6
						{
							let adj_orig = info
								.get(&adjacent_tile_id(
									turf_info.curr_transfer_dir as u8,
									*i,
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
				let mut progression_order: Vec<(usize, TurfMixture)> =
					Vec::with_capacity(planet_turfs.len());
				for (i, m) in planet_turfs.iter() {
					progression_order.push((*i, *m));
					let mut cur_info = info.get_mut(i).unwrap().get_mut();
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
								let adj = TURF_GASES.get(&loc).unwrap();
								if adj_info.last_slow_queue_cycle == queue_cycle_slow
									|| adj.value().planetary_atmos.is_some()
								{
									continue;
								}
								let (call_result_sender,call_result_receiver) = flume::bounded(0);
								call_sender
									.send((
										i as u32,
										BLOCKS_CALLER,
										"consider_firelocks",
										vec![ByondArg::Turf(loc as u32)],
										Some(call_result_sender),
									))
									.unwrap();
								call_result_receiver.recv().unwrap();
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
			}
			let info_to_send = turfs
				.iter()
				.map(|(i, m)| (*i, (*m, Cell::new(info.get(i).unwrap().get()))))
				.collect::<BTreeMap<usize, (TurfMixture, Cell<MonstermosInfo>)>>();
			turf_sender.send(info_to_send).unwrap();
		}
		EQUALIZATION_STEP.store(EQUALIZATION_FINALIZING,Ordering::Relaxed);
	});
	rayon::spawn(move || {
		let turf_receiver = MONSTERMOS_TURF_CHANNEL.1.clone();
		while EQUALIZATION_STEP.load(Ordering::Relaxed) < EQUALIZATION_DONE
		{
			let mut res = turf_receiver.recv_timeout(std::time::Duration::from_millis(1));
			while res.is_ok() {
				let turf_set = res.unwrap();
				for (i, (turf, monstermos_info)) in turf_set.iter() {
					finalize_eq(
						*i,
						turf,
						monstermos_info,
						&turf_set,
						max_x,
						max_y,
					);
				}
				res = turf_receiver.recv_timeout(std::time::Duration::from_micros(50));
			}
			EQUALIZATION_STEP.compare_and_swap(EQUALIZATION_FINALIZING, EQUALIZATION_DONE, Ordering::SeqCst);
		}
	});
	let ret_list = List::new();
	let mut done = false;
	let backoff = crossbeam::utils::Backoff::new();
	let final_receiver = EQUALIZE_FINALIZE_CHANNEL.1.clone();
	let call_receiver = CALL_CHANNEL.1.clone();
	let hpd_receiver = HIGH_PRESSURE_DELTA_CHANNEL.1.clone();
	while !done {
		let mut should_back_off = true;
		done = true;
		let mut call_res = call_receiver.try_recv();
		let mut finalize_res = final_receiver.try_recv();
		let mut hpd_res = hpd_receiver.try_recv();
		while call_res.is_ok() {
			done = false;
			should_back_off = false;
			let (turf_idx, flags, fn_name, args,call_result_sender) = call_res.unwrap();
			let turf = TurfGrid::turf_by_id(turf_idx as u32);
			match fn_name {
				"set" => turf.set(args[0].to_string().unwrap(), &args[1].to_usable_arg()),
				_ => {
					let true_args: Vec<Value> = args.iter().map(|v| v.to_usable_arg()).collect();
					if let Err(e) = turf.call(fn_name, &true_args) {
						src.call("stack_trace", &[&Value::from_string(e.message.as_str())])?;
					}
				}
			}
			if flags & BLOCKS_CALLER == BLOCKS_CALLER {
				call_result_sender.unwrap().send(true).unwrap();
			}
			call_res = call_receiver.try_recv();
		}
		while finalize_res.is_ok() {
			done = false;
			should_back_off = false;
			let (turf_idx, other_idx, amount) = finalize_res.unwrap();
			let real_amount = Value::from(amount);
			let turf = TurfGrid::turf_by_id(turf_idx as u32);
			let other_turf = TurfGrid::turf_by_id(other_idx as u32);
			if start_time.elapsed() > time_limit {
				let new_list = List::new();
				new_list.append(&other_turf);
				new_list.append(&real_amount);
				if let Err(e) = ret_list.set(&turf, &Value::from(new_list)) {
					src.call("stack_trace", &[&Value::from_string(e.message.as_str())])?;
				}
			} else {
				if let Err(e) = turf.call("update_visuals", &[Value::null()]) {
					src.call("stack_trace", &[&Value::from_string(e.message.as_str())])?;
				}
				if let Err(e) = other_turf.call("update_visuals", &[Value::null()]) {
					src.call("stack_trace", &[&Value::from_string(e.message.as_str())])?;
				}
				if let Err(e) =
					turf.call("consider_pressure_difference", &[other_turf, real_amount])
				{
					src.call("stack_trace", &[&Value::from_string(e.message.as_str())])?;
				}
			}
			finalize_res = final_receiver.try_recv();
		}
		while hpd_res.is_ok() {
			done = false;
			should_back_off = false;
			let hpd: List = src.get_list("high_pressure_delta")?;
			let true_value = Value::from(1.0);
			for hpd_turf in hpd_res.unwrap().iter().map(|item| item.to_usable_arg()) {
				hpd.set(&hpd_turf, &true_value)?;
			}
			hpd_res = hpd_receiver.try_recv();
		}
		if should_back_off {
			backoff.snooze();
		} else {
			backoff.reset();
		}
		done = done && EQUALIZATION_STEP.compare_and_swap(EQUALIZATION_DONE, EQUALIZATION_NONE, Ordering::Relaxed) == EQUALIZATION_DONE;
	}
	Ok(Value::from(ret_list))
}

#[hook("/datum/controller/subsystem/air/proc/process_turf_equalize_extools")]
fn _hook_equalize() {
	actual_equalize(src, args)
}
