use super::*;

use std::collections::VecDeque;

use std::collections::{BTreeMap, BTreeSet};

use std::cell::Cell;

use auxcallback::{callback_sender_by_id_insert, process_callbacks_for_millis};

use std::sync::atomic::{AtomicU8, Ordering};

const EQUALIZATION_NONE: u8 = 0;
const EQUALIZATION_PROCESSING: u8 = 1;
const EQUALIZATION_DONE: u8 = 2;

static EQUALIZATION_STEP: AtomicU8 = AtomicU8::new(0);

const OPP_DIR_INDEX: [u8; 7] = [1, 0, 3, 2, 5, 4, 6];

// If you can't tell, this is mostly a massively simplified copy of monstermos.

fn explosively_depressurize(
	ctx: &DMContext,
	turf_idx: TurfID,
	turf: TurfMixture,
	equalize_hard_turf_limit: usize,
	max_x: i32,
	max_y: i32,
) -> DMResult {
	let mut turfs: Vec<(TurfID, TurfMixture)> = Vec::new();
	let mut space_turfs: Vec<(TurfID, TurfMixture)> = Vec::new();
	turfs.push((turf_idx, turf));
	let mut warned_about_planet_atmos = false;
	let mut cur_queue_idx = 0;
	while cur_queue_idx < turfs.len() {
		let (i, m) = turfs[cur_queue_idx];
		let actual_turf = unsafe { Value::turf_by_id_unchecked(i) };
		cur_queue_idx += 1;
		if m.planetary_atmos.is_some() {
			warned_about_planet_atmos = true;
			continue;
		}
		if m.is_immutable() {
			space_turfs.push((i, m));
			actual_turf.set("pressure_specific_target", &actual_turf);
		} else {
			if cur_queue_idx > equalize_hard_turf_limit {
				continue;
			}
			for (j, loc) in adjacent_tile_ids(m.adjacency, i, max_x, max_y) {
				actual_turf.call(
					"consider_firelocks",
					&[&unsafe { Value::turf_by_id_unchecked(loc) }],
				)?;
				let new_m = TURF_GASES.get(&i).unwrap();
				let bit = 1 << j;
				if new_m.adjacency & bit == bit {
					if let Some(adj) = TURF_GASES.get(&loc) {
						let (&adj_i, &adj_m) = (adj.key(), adj.value());
						turfs.push((adj_i, adj_m));
					}
				}
			}
		}
		if warned_about_planet_atmos {
			return Ok(Value::null()); // planet atmos > space
		}
	}
	let mut progression_order: Vec<(TurfID, TurfMixture)> = Vec::with_capacity(space_turfs.len());
	let mut adjacency_info: BTreeMap<TurfID, Cell<(u8, f32)>> = BTreeMap::new();
	for (i, m) in space_turfs.iter() {
		progression_order.push((*i, *m));
		adjacency_info.insert(*i, Cell::new((6, 0.0)));
	}
	cur_queue_idx = 0;
	while cur_queue_idx < progression_order.len() {
		let (i, m) = progression_order[cur_queue_idx];
		let actual_turf = unsafe { Value::turf_by_id_unchecked(i) };
		for (j, loc) in adjacent_tile_ids(m.adjacency, i, max_x, max_y) {
			if let Some(adj) = TURF_GASES.get(&loc) {
				let (adj_i, adj_m) = (*adj.key(), adj.value());
				if !adjacency_info.contains_key(&adj_i) && !adj_m.is_immutable() {
					adjacency_info.insert(i, Cell::new((OPP_DIR_INDEX[j as usize], 0.0)));
					unsafe { Value::turf_by_id_unchecked(adj_i) }
						.set("pressure_specific_target", &actual_turf);
					progression_order.push((adj_i, *adj_m));
				}
			}
		}
		cur_queue_idx += 1;
	}
	let hpd = ctx.get_global("SSAir")?.get_list("high_pressure_delta")?;
	for (i, m) in progression_order.iter().rev() {
		let cur_orig = adjacency_info.get(i).unwrap();
		let mut cur_info = cur_orig.get();
		if cur_info.0 == 6 {
			continue;
		}
		let actual_turf = unsafe { Value::turf_by_id_unchecked(*i) };
		hpd.set(&actual_turf, 1.0)?;
		let loc = adjacent_tile_id(cur_info.0, *i, max_x, max_y);
		if let Some(adj) = TURF_GASES.get(&loc) {
			let (adj_i, adj_m) = (*adj.key(), adj.value());
			let adj_orig = adjacency_info.get(&adj_i).unwrap();
			let mut adj_info = adj_orig.get();
			let sum = adj_m.total_moles();
			cur_info.1 += sum;
			adj_info.1 += cur_info.1;
			if adj_info.0 != 6 {
				let adj_turf = unsafe { Value::turf_by_id_unchecked(adj_i) };
				adj_turf.set("pressure_difference", cur_info.1);
				adj_turf.set("pressure_direction", (1 << cur_info.0) as f32);
			}
			m.clear_air();
			actual_turf.set("pressure_difference", cur_info.1);
			actual_turf.set("pressure_direction", (1 << cur_info.0) as f32);
			actual_turf.call("handle decompression floor rip", &[&Value::from(sum)])?;
			adj_orig.set(adj_info);
			cur_orig.set(cur_info);
		}
	}
	Ok(Value::null())
}

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
	if !resumed
		&& !turf_receiver.is_empty()
		&& EQUALIZATION_STEP.compare_and_swap(
			EQUALIZATION_NONE,
			EQUALIZATION_PROCESSING,
			Ordering::SeqCst,
		) == EQUALIZATION_NONE
	{
		rayon::spawn(move || {
			let sender = callback_sender_by_id_insert(SSAIR_NAME.to_string());
			'turf_loop: for initial_idx in turf_receiver.try_iter() {
				if let Some(initial_turf) = TURF_GASES.get(&initial_idx) {
					if initial_turf.simulation_level == SIMULATION_LEVEL_ALL
						&& initial_turf.adjacency > 0
					{
						let our_moles = initial_turf.total_moles();
						if our_moles < 10.0 {
							continue;
						}
					} else {
						continue;
					}
					let mut found_turfs: BTreeSet<TurfID> = BTreeSet::new();
					let mut turfs: Vec<(TurfID, TurfMixture, TurfID, f32)> =
						Vec::with_capacity(equalize_turf_limit);
					let mut border_turfs: VecDeque<(TurfID, TurfMixture, TurfID, f32)> =
						VecDeque::with_capacity(equalize_turf_limit);
					let mut final_mix = GasMixture::new();
					border_turfs.push_back((initial_idx, *initial_turf, initial_idx, 0.0));
					let mut was_space = false;
					GasMixtures::with_all_mixtures(|all_mixtures| {
						while border_turfs.len() > 0 && turfs.len() < equalize_turf_limit {
							let (cur_idx, cur_turf, parent_turf, pressure_delta) =
								border_turfs.pop_front().unwrap();
							if let Some(entry) = all_mixtures.get(cur_turf.mix) {
								let gas: &GasMixture = &entry.read();
								final_mix.merge(gas);
								final_mix.volume += gas.volume;
							}
							turfs.push((cur_idx, cur_turf, parent_turf, pressure_delta));
							for (_, loc) in
								adjacent_tile_ids(cur_turf.adjacency, cur_idx, max_x, max_y)
							{
								if found_turfs.contains(&loc) {
									continue;
								}
								if let Some(adj_turf) = TURF_GASES.get(&loc) {
									if let Some(entry) = all_mixtures.get(adj_turf.mix) {
										let gas: &GasMixture = &entry.read();
										if cfg!(putnamos_decompression) && gas.is_immutable() {
											let _ = sender.try_send(Box::new(move |new_ctx| {
												explosively_depressurize(
													new_ctx,
													cur_idx,
													cur_turf,
													equalize_hard_turf_limit,
													max_x,
													max_y,
												)
											}));
											was_space = true;
											return;
										} else {
											let delta =
												gas.return_pressure() - final_mix.return_pressure();
											if delta < 0.0 {
												border_turfs.push_back((
													loc,
													*adj_turf.value(),
													cur_idx,
													-delta,
												));
											}
										}
									}
								}
							}
							found_turfs.insert(cur_idx);
						}
					});
					if was_space {
						continue 'turf_loop;
					}
					final_mix.multiply(1.0 / turfs.len() as f32);
					GasMixtures::with_all_mixtures(|all_mixtures| {
						for (cur_idx, cur_turf, parent_turf, pressure_delta) in turfs.iter() {
							if let Some(entry) = all_mixtures.get(cur_turf.mix) {
								let gas: &mut GasMixture = &mut entry.write();
								gas.copy_from_mutable(&final_mix);
							}
							let idx_copy = *cur_idx;
							let parent_copy = *parent_turf;
							let actual_delta = *pressure_delta;
							let _ = sender.try_send(Box::new(move |_| {
								if parent_copy != 0 {
									let turf = unsafe { Value::turf_by_id_unchecked(idx_copy) };
									let enemy_turf =
										unsafe { Value::turf_by_id_unchecked(parent_copy) };
									enemy_turf.call(
										"consider_pressure_difference",
										&[&turf, &Value::from(actual_delta)],
									)?;
								}
								Ok(Value::null())
							}));
						}
					});
				}
			}
			EQUALIZATION_STEP.store(EQUALIZATION_DONE, Ordering::Relaxed);
		});
	}
	let arg_limit = args
		.get(1)
		.ok_or_else(|| runtime!("Wrong number of arguments to turf equalization: 1"))?
		.as_number()?;
	if arg_limit <= 0.0 {
		return Ok(Value::from(true));
	}
	process_callbacks_for_millis(ctx, SSAIR_NAME.to_string(), arg_limit as u64);
	let prev_value =
		EQUALIZATION_STEP.compare_and_swap(EQUALIZATION_DONE, EQUALIZATION_NONE, Ordering::SeqCst);
	Ok(Value::from(
		prev_value != EQUALIZATION_DONE && prev_value != EQUALIZATION_NONE,
	))
}

// Expected function call: process_turf_equalize_extools((Master.current_ticklimit - TICK_USAGE) * world.tick_lag)
// Returns: TRUE if not done, FALSE if done
#[hook("/datum/controller/subsystem/air/proc/process_turf_equalize_auxtools")]
fn _hook_equalize() {
	actual_equalize(src, args, ctx)
}
