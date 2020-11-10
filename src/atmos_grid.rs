use super::gas::gas_mixture::GasMixture;

use super::gas::GasMixtures;

use turf_grid::*;

use dm::*;

use super::gas::constants::*;

use std::collections::VecDeque;

use dashmap::DashMap;

use std::collections::{BTreeMap, BTreeSet};

use std::cell::Cell;

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

const SIMULATION_LEVEL_NONE: u8 = 0;
//const SIMULATION_LEVEL_SHARE_FROM: u8 = 1;
const SIMULATION_LEVEL_SIMULATE: u8 = 2;

use crossbeam::channel;
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

// gas id, bitflags (1 = should update visuals, 2 = should react)
type TurfProcessInfo = Option<(u32, u32)>;

lazy_static! {
	static ref TURF_PROCESS_CHANNEL: (
		channel::Sender<TurfProcessInfo>,
		channel::Receiver<TurfProcessInfo>
	) = channel::unbounded();
}

#[hook("/datum/controller/subsystem/air/proc/begin_turf_process")]
fn _begin_turfs_hook() {
	let sender = TURF_PROCESS_CHANNEL.0.clone();
	let (gas_sender, gas_receiver) = mpsc::channel();
	rayon::spawn(move || {
		let max_x = TurfGrid::max_x();
		let max_y = TurfGrid::max_y();
		for e in TURF_GASES
			.iter()
			.filter(|e| e.value().simulation_level >= SIMULATION_LEVEL_SIMULATE)
		{
			let (i, m) = (*e.key(), *e.value());
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
			let adj_tiles = adjacent_tile_ids(adj, i, max_x, max_y);
			GasMixtures::with_all_mixtures(|all_mixtures| {
				for loc in adj_tiles.iter() {
					if let Some(gas) = TURF_GASES.get(loc) {
						end_gas.merge(all_mixtures.get(gas.mix).unwrap());
					}
				}
			});
			let mut is_planet = false; // not a bool because it's literally used as an integer
			if let Some(planet_atmos) = m.planetary_atmos {
				end_gas.merge(PLANETARY_ATMOS.get(planet_atmos).unwrap().value());
				is_planet = true;
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
			end_gas.multiply(GAS_DIFFUSION_CONSTANT);
			// Rather than immediately setting it, we send it to the thread defined below to be set when it's safe to do so.
			gas_sender.send(Some((i, m.mix, adj, end_gas,is_planet))).unwrap();
		}
		gas_sender.send(None).unwrap();
	});
	rayon::spawn(move || {
		// This closure is what sets the gas. It locks the mixtures, saves it, then sends the "we did stuff" data to the main thread.
		let write_to_gas = |(i, mix, adj, end_gas, is_planet): (usize, usize, i8, GasMixture, bool)| {
			let mut gas_copy = GasMixture::new(); // so we don't lock the mutex the *entire* time
			let mut flags = 0;
			let adj_amount = adj.count_ones() + (is_planet as u32);
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
			if flags > 0 {
				sender.send(Some((i as u32, flags))).unwrap();
			}
		};
		let max_north = TurfGrid::max_x();
		let max_up = TurfGrid::max_y() * max_north + 1;
		/*
			The above two and the below are what ensures the "safety"
			of the operation--it allows for the gas mixtures
			to be set in parallel, and for updating to happen in parallel,
			without causing inconsistent results, by only setting gas mixtures
			that are *definitely not* going to be read again.
		*/
		let mut min_diff = max_north + 1;
		let mut chunk_change: VecDeque<(usize, usize, i8, GasMixture, bool)> =
			VecDeque::with_capacity(100);
		for res in gas_receiver.iter() {
			if let Some((i, mix, adj, end_gas, is_planet)) = res {
				if adj & 32 == 32 {
					min_diff = max_up;
				}
				chunk_change.push_back((i, mix, adj, end_gas, is_planet));
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
			} else {
				break;
			}
		}
		// The processing thread is done--now we just do the rest.
		for t in chunk_change.drain(..) {
			write_to_gas(t);
		}
		sender.send(None).unwrap();
	});
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
	let time_limit = Duration::from_secs_f32(
		args.get(0)
			.ok_or_else(|| runtime!("Wrong number of arguments to process_turfs_extools: 0"))?
			.as_number()?,
	);
	let start_time = Instant::now();
	let receiver = TURF_PROCESS_CHANNEL.1.clone();
	let mut done = false;
	while start_time.elapsed() < time_limit && !done {
		if let Ok(res) = receiver.try_recv() {
			if let Some((i, flags)) = res {
				let turf = TurfGrid::turf_by_id(i);
				if flags & 2 == 2 {
					if let Err(e) = turf.get("air").unwrap().call("react", &[turf.clone()]) {
						src.call("stack_trace", &[&Value::from_string(e.message.as_str())])?;
					}
				}
				if flags & 1 == 1 {
					if let Err(e) = turf.call("update_visuals", &[Value::null()]) {
						src.call("stack_trace", &[&Value::from_string(e.message.as_str())])?;
					}
				}
			} else {
				done = true;
			}
		}
	}
	Ok(Value::from(if start_time.elapsed() < time_limit {
		0.0
	} else {
		1.0
	}))
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
#[cfg(explosive_decompression)]
type ByondMessage<'a> = (u32, u32, &'a str, Vec<ByondArg>);

fn finalize_eq(
	i: usize,
	turf: &TurfMixture,
	monstermos_orig: &Cell<MonstermosInfo>,
	other_turfs: &BTreeMap<usize, (TurfMixture, Cell<MonstermosInfo>)>,
	max_x: i32,
	max_y: i32,
	sender: &std::sync::mpsc::Sender<(usize, usize, f32)>,
) {
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
			finalize_eq_neighbors(i, turf, monstermos_orig, other_turfs, max_x, max_y, sender);
			monstermos_info = monstermos_orig.get();
		}
		GasMixtures::with_all_mixtures_mut(|all_mixtures| {
			let air = all_mixtures.get_mut(turf.mix).unwrap();
			air.remove(planet_transfer_amount);
		});
	} else if planet_transfer_amount < 0.0 {
		if let Some(air_entry) = PLANETARY_ATMOS.get(turf.planetary_atmos.unwrap()) {
			let planet_air = air_entry.value();
			let planet_sum = planet_air.total_moles();
			if planet_sum > 0.0 {
				GasMixtures::with_all_mixtures_mut(|all_mixtures| {
					let air = all_mixtures.get_mut(turf.mix).unwrap();
					air.merge(&(planet_air * (-planet_transfer_amount / planet_sum)));
				});
			}
		}
	}
	for j in 0..6 {
		let bit = 1 << j;
		if turf.adjacency & bit == bit {
			let amount = monstermos_info.transfer_dirs[j as usize];
			if amount > 0.0 {
				if turf.total_moles() < amount {
					finalize_eq_neighbors(
						i,
						turf,
						monstermos_orig,
						other_turfs,
						max_x,
						max_y,
						sender,
					);
					monstermos_info = monstermos_orig.get();
				}
				let adj_id = adjacent_tile_id(j as u8, i, max_x, max_y);
				let (adj_turf, adj_orig) = other_turfs.get(&adj_id).unwrap();
				let mut adj_info = adj_orig.take();
				adj_info.transfer_dirs[OPP_DIR_INDEX[j as usize]] = 0.0;
				if turf.mix > adj_turf.mix {
					let split_idx = adj_turf.mix + 1;
					GasMixtures::with_all_mixtures_mut(|all_mixtures| {
						let (left, right) = all_mixtures.split_at_mut(split_idx);
						let other_air = left.last_mut().unwrap();
						let air = right.get_mut(turf.mix - split_idx).unwrap();
						other_air.merge(&air.remove(amount));
					});
				} else if turf.mix < adj_turf.mix {
					let split_idx = turf.mix + 1;
					GasMixtures::with_all_mixtures_mut(|all_mixtures| {
						let (left, right) = all_mixtures.split_at_mut(split_idx);
						let air = left.last_mut().unwrap();
						let other_air = right.get_mut(adj_turf.mix - split_idx).unwrap();
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

fn finalize_eq_neighbors(
	i: usize,
	turf: &TurfMixture,
	monstermos_info: &Cell<MonstermosInfo>,
	other_turfs: &BTreeMap<usize, (TurfMixture, Cell<MonstermosInfo>)>,
	max_x: i32,
	max_y: i32,
	sender: &std::sync::mpsc::Sender<(usize, usize, f32)>,
) {
	let monstermos_copy = monstermos_info.get();
	for j in 0..6 {
		let amount = monstermos_copy.transfer_dirs[i];
		let bit = 1 << j;
		if amount < 0.0 && turf.adjacency & bit == bit {
			let adjacent_id = adjacent_tile_id(j, i, max_x, max_y);
			if let Some((other_turf, other_info)) = other_turfs.get(&adjacent_id) {
				finalize_eq(
					adjacent_id,
					other_turf,
					other_info,
					other_turfs,
					max_x,
					max_y,
					sender,
				);
			}
		}
	}
}

#[cfg(explosive_decompression)]
fn explosively_depressurize(
	turf_idx: usize,
	turf: TurfMixture,
	info: &mut BTreeMap<usize, Cell<MonstermosInfo>>,
	queue_cycle_slow: &mut i32,
	monstermos_hard_turf_limit: usize,
	call_sender: &std::sync::mpsc::Sender<ByondMessage>,
	call_result_receiver: &std::sync::mpsc::Receiver<i32>,
	hpd_sender: &std::sync::mpsc::Sender<Vec<ByondArg>>,
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
					let &(adj_i, adj_m) = turfs.get(loc).unwrap();
					let adj_orig = info.get(&loc).unwrap();
					let mut adj_info = adj_orig.get();
					if adj_info.done_this_cycle {
						continue;
					}
					call_sender
						.send((
							i as u32,
							BLOCKS_CALLER,
							"consider_firelocks",
							vec![ByondArg::Turf(loc as u32)],
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
				let adj_orig = info.get(&loc).unwrap();
				let mut adj_info = adj_orig.get();
				if adj_info.done_this_cycle
					&& adj_info.last_slow_queue_cycle == *queue_cycle_slow
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
				))
				.unwrap();
		}
		m.clear_air();
		call_sender
			.send((*i as u32, 0, "update_visuals", vec![ByondArg::Null]))
			.unwrap();
		call_sender
			.send((
				*i as u32,
				0,
				"handle decompression floor rip",
				vec![ByondArg::Float(sum)],
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
	let time_limit = Duration::from_secs_f32(
		args.get(0)
			.ok_or_else(|| runtime!("Wrong number of arguments to process_turfs_extools: 0"))?
			.as_number()?,
	);
	let start_time = Instant::now();
	let max_x = TurfGrid::max_x();
	let max_y = TurfGrid::max_y();
	let (final_sender, final_receiver) = mpsc::channel();
	let (hpd_sender, hpd_receiver): (
		std::sync::mpsc::Sender<Vec<ByondArg>>,
		std::sync::mpsc::Receiver<Vec<ByondArg>>,
	) = mpsc::channel();
	let (call_sender, call_receiver) = mpsc::channel();
	let (call_result_sender, call_result_receiver) = mpsc::channel();
	let (turf_sender, turf_receiver) = mpsc::channel();
	rayon::spawn(move || {
		let mut info: BTreeMap<usize, Cell<MonstermosInfo>> = BTreeMap::new();
		let mut queue_cycle_slow = 0;
		for e in TURF_GASES.iter() {
			let (i, m) = (e.key(), e.value());
			if m.simulation_level >= SIMULATION_LEVEL_SIMULATE {
				if let Some(our_info) = info.get(i) {
					if our_info.get().done_this_cycle {
						continue;
					}
				}
				let adj_tiles = adjacent_tile_ids(m.adjacency, *i, max_x, max_y);
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
					#[cfg(explosive_decompression)]
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
									&call_sender,
									&call_result_receiver,
									&hpd_sender,
								);
								space_this_time = true;
							}
						}
					}
					#[cfg(not(explosive_decompression))]
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
							let adj_orig =
								info.get(&(turf_info.curr_transfer_dir as usize)).unwrap();
							let mut adj_info = adj_orig.get();
							turf_info.adjust_eq_movement(
								&mut adj_info,
								turf_info.curr_transfer_dir,
								turf_info.curr_transfer_amount,
							);
							adj_info.curr_transfer_amount += turf_info.curr_transfer_amount;
							turf_info.curr_transfer_amount = 0.0;
							turf_orig.set(turf_info);
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
							let adj_orig =
								info.get(&(turf_info.curr_transfer_dir as usize)).unwrap();
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
								call_sender
									.send((
										i as u32,
										BLOCKS_CALLER,
										"consider_firelocks",
										vec![ByondArg::Turf(loc as u32)],
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
		drop(turf_sender);
		drop(call_sender);
		drop(hpd_sender);
	});
	rayon::spawn(move || {
		for turf_set in turf_receiver.recv() {
			for (i, (turf, monstermos_info)) in turf_set.iter() {
				finalize_eq(
					*i,
					turf,
					monstermos_info,
					&turf_set,
					max_x,
					max_y,
					&final_sender,
				);
			}
		}
		drop(final_sender);
	});
	let mut any_err: DMResult = Ok(Value::null());
	let ret_list = List::new();
	let mut done = false;
	let backoff = crossbeam::utils::Backoff::new();
	while !done {
		let mut should_back_off = true;
		done = true;
		let call_res = call_receiver.try_recv();
		let finalize_res = final_receiver.try_recv();
		let hpd_res = hpd_receiver.try_recv();
		if call_res.is_ok() {
			done = false;
			should_back_off = false;
			let (turf_idx, flags, fn_name, args) = call_res.unwrap();
			let turf = TurfGrid::turf_by_id(turf_idx as u32);
			match fn_name {
				"set" => turf.set(args[0].to_string().unwrap(), &args[1].to_usable_arg()),
				_ => {
					let true_args: Vec<Value> = args.iter().map(|v| v.to_usable_arg()).collect();
					if let Err(e) = turf.call(fn_name, &true_args) {
						any_err = Err(e);
					}
				}
			}
			if flags & BLOCKS_CALLER == BLOCKS_CALLER {
				call_result_sender.send(0).unwrap();
			}
		} else if let Err(x) = call_res {
			done = done && x == std::sync::mpsc::TryRecvError::Disconnected;
		}
		if finalize_res.is_ok() {
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
					any_err = Err(e);
				}
			} else {
				if let Err(e) = turf.call("update_visuals", &[Value::null()]) {
					any_err = Err(e);
				}
				if let Err(e) = other_turf.call("update_visuals", &[Value::null()]) {
					any_err = Err(e);
				}
				if let Err(e) =
					turf.call("consider_pressure_difference", &[other_turf, real_amount])
				{
					any_err = Err(e);
				}
			}
		} else if let Err(x) = finalize_res {
			done = done && x == std::sync::mpsc::TryRecvError::Disconnected;
		}
		if hpd_res.is_ok() {
			let hpd: List = Value::globals()
				.get("SSAir")?
				.get_list("high_pressure_delta")?;
			let true_value = Value::from(1.0);
			for hpd_turf in hpd_res.unwrap().iter().map(|item| item.to_usable_arg()) {
				hpd.set(&hpd_turf, &true_value)?;
			}
		} else if let Err(x) = hpd_res {
			done = done && x == std::sync::mpsc::TryRecvError::Disconnected;
		}
		if should_back_off {
			backoff.snooze();
		} else {
			backoff.reset();
		}
	}
	if let Err(e) = any_err {
		src.call("stack_trace", &[&Value::from_string(e.message.as_str())])
			.unwrap();
	}
	Ok(Value::from(ret_list))
}

#[hook("/datum/controller/subsystem/air/proc/process_turf_equalize_extools")]
fn _hook_equalize() {
	actual_equalize(src, args)
}
