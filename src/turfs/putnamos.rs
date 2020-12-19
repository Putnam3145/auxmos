use super::*;

use std::collections::VecDeque;

use std::collections::BTreeSet;

use auxcallback::{callback_sender_by_id_insert, process_callbacks_for_millis};

use std::sync::atomic::{AtomicU8, Ordering};

const EQUALIZATION_NONE: u8 = 0;
const EQUALIZATION_PROCESSING: u8 = 1;
const EQUALIZATION_DONE: u8 = 2;

static EQUALIZATION_STEP: AtomicU8 = AtomicU8::new(0);

// If you can't tell, this is mostly a massively simplified copy of monstermos.

#[cfg(feature = "explosive_decompression")]
fn explosively_depressurize(
	turf_idx: TurfID,
	turf: TurfMixture,
	equalize_hard_turf_limit: usize,
	max_x: i32,
	max_y: i32,
) {
}

fn actual_equalize(src: &Value, args: &[Value], ctx: &DMContext) -> DMResult {
	let equalize_turf_limit = src.get_number("equalize_turf_limit")? as usize;
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
			for initial_idx in turf_receiver.try_iter() {
				if let Some(initial_turf) = TURF_GASES.get(&initial_idx) {
					if initial_turf.simulation_level >= SIMULATION_LEVEL_ALL
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
								adjacent_tile_ids(cur_turf.adjacency, cur_idx, max_x, max_y).iter()
							{
								if found_turfs.contains(loc) {
									continue;
								}
								if let Some(adj_turf) = TURF_GASES.get(loc) {
									if let Some(entry) = all_mixtures.get(adj_turf.mix) {
										let gas: &GasMixture = &entry.read();
										let delta =
											gas.return_pressure() - final_mix.return_pressure();
										if delta < 0.0 {
											border_turfs.push_back((
												*loc,
												*adj_turf.value(),
												cur_idx,
												-delta,
											));
										}
									}
								}
							}
							found_turfs.insert(cur_idx);
						}
					});
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
								let turf = unsafe { Value::turf_by_id_unchecked(idx_copy) };
								let enemy_turf =
									unsafe { Value::turf_by_id_unchecked(parent_copy) };
								turf.call(
									"consider_pressure_difference",
									&[&enemy_turf, &Value::from(actual_delta)],
								)?;
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
#[hook("/datum/controller/subsystem/air/proc/process_turf_equalize_extools")]
fn _hook_equalize() {
	actual_equalize(src, args, ctx)
}
