const PROCESS_NOT_STARTED: u8 = 0;

const PROCESS_PROCESSING: u8 = 1;

const PROCESS_DONE: u8 = 2;

use std::collections::{BTreeSet, VecDeque};

use auxtools::*;

use super::*;

static PROCESSING_TURF_STEP: AtomicU8 = AtomicU8::new(PROCESS_NOT_STARTED);

use crate::GasMixtures;

use std::time::{Duration, Instant};

use auxcallback::{byond_callback_sender, process_callbacks_for_millis};

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

static WAITING_FOR_THREAD: AtomicBool = AtomicBool::new(false);

#[hook("/datum/controller/subsystem/air/proc/thread_running")]
fn _thread_running_hook() {
	Ok(Value::from(
		PROCESSING_TURF_STEP.load(Ordering::Relaxed) == PROCESS_PROCESSING,
	))
}

#[hook("/datum/controller/subsystem/air/proc/finish_turf_processing_auxtools")]
fn _finish_process_turfs() {
	WAITING_FOR_THREAD.store(true, Ordering::SeqCst);
	let arg_limit = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of arguments to turf finishing: 0"))?
		.as_number()
		.map_err(|_| {
			runtime!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})?;
	let processing_callbacks_unfinished = process_callbacks_for_millis(arg_limit as u64);
	// If PROCESSING_TURF_STEP is done, we're done, and we should set it to NOT_STARTED while we're at it.
	let processing_turfs_unfinished = PROCESSING_TURF_STEP.compare_exchange(
		PROCESS_DONE,
		PROCESS_NOT_STARTED,
		Ordering::SeqCst,
		Ordering::Relaxed,
	) == Err(PROCESS_PROCESSING);
	if processing_callbacks_unfinished || processing_turfs_unfinished {
		Ok(Value::from(true))
	} else {
		WAITING_FOR_THREAD.store(false, Ordering::SeqCst);
		Ok(Value::from(false))
	}
}

#[hook("/datum/controller/subsystem/air/proc/process_turfs_auxtools")]
fn _process_turf_hook() {
	let resumed = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of arguments to turf processing: 0"))?
		.as_number()
		.map_err(|_| {
			runtime!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})? == 1.0;
	#[allow(unused_variables)]
	if !resumed && PROCESSING_TURF_STEP.load(Ordering::SeqCst) == PROCESS_NOT_STARTED {
		// Don't want to start it while there's already a thread running, so we only start it if it hasn't been started.
		let fdm_max_steps = src
			.get_number(byond_string!("share_max_steps"))
			.unwrap_or_else(|_| 1.0) as i32;
		let equalize_turf_limit = src
			.get_number(byond_string!("equalize_turf_limit"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})? as usize;
		let equalize_hard_turf_limit = src
			.get_number(byond_string!("equalize_hard_turf_limit"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})? as usize;
		let equalize_enabled = cfg!(feature = "equalization")
			&& src
				.get_number(byond_string!("equalize_enabled"))
				.map_err(|_| {
					runtime!(
						"Attempt to interpret non-number value as number {} {}:{}",
						std::file!(),
						std::line!(),
						std::column!()
					)
				})? != 0.0;
		let group_pressure_goal = src
			.get_number(byond_string!("excited_group_pressure_goal"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?;
		let max_x = auxtools::Value::world()
			.get_number(byond_string!("maxx"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})? as i32;
		let max_y = auxtools::Value::world()
			.get_number(byond_string!("maxy"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})? as i32;
		rayon::spawn(move || {
			PROCESSING_TURF_STEP.store(PROCESS_PROCESSING, Ordering::SeqCst);
			let sender = byond_callback_sender();
			let (low_pressure_turfs, high_pressure_turfs) = {
				let start_time = Instant::now();
				let (low_pressure_turfs, high_pressure_turfs) = fdm(max_x, max_y, fdm_max_steps);
				let bench = start_time.elapsed().as_millis();
				let (lpt, hpt) = (low_pressure_turfs.len(), high_pressure_turfs.len());
				let _ = sender.try_send(Box::new(move || {
					let ssair = auxtools::Value::globals().get(byond_string!("SSair"))?;
					let prev_cost =
						ssair.get_number(byond_string!("cost_turfs")).map_err(|_| {
							runtime!(
								"Attempt to interpret non-number value as number {} {}:{}",
								std::file!(),
								std::line!(),
								std::column!()
							)
						})?;
					ssair.set(
						byond_string!("cost_turfs"),
						Value::from(0.8 * prev_cost + 0.2 * (bench as f32)),
					)?;
					ssair.set(byond_string!("low_pressure_turfs"), Value::from(lpt as f32))?;
					ssair.set(
						byond_string!("high_pressure_turfs"),
						Value::from(hpt as f32),
					)?;
					Ok(Value::null())
				}));
				(low_pressure_turfs, high_pressure_turfs)
			};
			{
				let start_time = Instant::now();
				let processed_turfs =
					excited_group_processing(max_x, max_y, group_pressure_goal, low_pressure_turfs);
				let bench = start_time.elapsed().as_millis();
				let _ = sender.try_send(Box::new(move || {
					let ssair = auxtools::Value::globals().get(byond_string!("SSair"))?;
					let prev_cost =
						ssair
							.get_number(byond_string!("cost_groups"))
							.map_err(|_| {
								runtime!(
									"Attempt to interpret non-number value as number {} {}:{}",
									std::file!(),
									std::line!(),
									std::column!()
								)
							})?;
					ssair.set(
						byond_string!("cost_groups"),
						Value::from(0.8 * prev_cost + 0.2 * (bench as f32)),
					)?;
					ssair.set(
						byond_string!("num_group_turfs_processed"),
						Value::from(processed_turfs as f32),
					)?;
					Ok(Value::null())
				}));
			}
			if equalize_enabled {
				let start_time = Instant::now();
				let processed_turfs = {
					#[cfg(feature = "putnamos")]
					{
						super::putnamos::equalize(
							equalize_turf_limit,
							equalize_hard_turf_limit,
							max_x,
							max_y,
							high_pressure_turfs,
						)
					}
					#[cfg(feature = "monstermos")]
					{
						super::monstermos::equalize(
							equalize_turf_limit,
							equalize_hard_turf_limit,
							max_x,
							max_y,
							high_pressure_turfs,
						)
					}
					#[cfg(not(feature = "equalization"))]
					{
						0
					}
				};
				let bench = start_time.elapsed().as_millis();
				let _ = sender.try_send(Box::new(move || {
					let ssair = auxtools::Value::globals().get(byond_string!("SSair"))?;
					let prev_cost =
						ssair
							.get_number(byond_string!("cost_equalize"))
							.map_err(|_| {
								runtime!(
									"Attempt to interpret non-number value as number {} {}:{}",
									std::file!(),
									std::line!(),
									std::column!()
								)
							})?;
					ssair.set(
						byond_string!("cost_equalize"),
						Value::from(0.8 * prev_cost + 0.2 * (bench as f32)),
					)?;
					ssair.set(
						byond_string!("num_equalize_processed"),
						Value::from(processed_turfs as f32),
					)?;
					Ok(Value::null())
				}));
			}
			{
				let start_time = Instant::now();
				post_process();
				let bench = start_time.elapsed().as_millis();
				let _ = sender.try_send(Box::new(move || {
					let ssair = auxtools::Value::globals().get(byond_string!("SSair"))?;
					let prev_cost = ssair
						.get_number(byond_string!("cost_post_process"))
						.map_err(|_| {
							runtime!(
								"Attempt to interpret non-number value as number {} {}:{}",
								std::file!(),
								std::line!(),
								std::column!()
							)
						})?;
					ssair.set(
						byond_string!("cost_post_process"),
						Value::from(0.8 * prev_cost + 0.2 * (bench as f32)),
					)?;
					Ok(Value::null())
				}));
			}
			PROCESSING_TURF_STEP.store(PROCESS_DONE, Ordering::SeqCst);
		});
	}
	Ok(Value::from(false))
}

fn fdm(max_x: i32, max_y: i32, fdm_max_steps: i32) -> (BTreeSet<TurfID>, BTreeSet<TurfID>) {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature,
		sleeping turfs, which is why I've renamed it to fdm.
	*/
	PROCESSING_TURF_STEP.store(PROCESS_PROCESSING, Ordering::SeqCst);
	let mut low_pressure_turfs: BTreeSet<TurfID> = BTreeSet::new();
	let mut high_pressure_turfs: BTreeSet<TurfID> = BTreeSet::new();
	let mut cur_count = 1;
	loop {
		if cur_count > fdm_max_steps || WAITING_FOR_THREAD.load(Ordering::SeqCst) {
			break;
		}
		GasMixtures::with_all_mixtures(|all_mixtures| {
			let turfs_to_save = turf_gases()
				/*
					This uses the DashMap raw API to access the shards directly.
					This allows for it to be parallelized much more efficiently
					with rayon; the speedup gained from this is actually linear
					with the amount of cores the CPU has, which, to be frank,
					is way better than I was expecting, even though this operation
					is technically embarassingly parallel. It'll probably reach
					some maximum due to the global turf mixture lock access,
					but it's already blazingly fast on my i7, so it should be fine.
				*/
				.shards()
				.par_iter()
				.map(|shard| {
					shard
						.read()
						.iter()
						.filter(|(&i, m_v)| {
							let m = *m_v.get();
							let adj = m.adjacency;
							if m.simulation_level > SIMULATION_LEVEL_NONE
								&& adj > 0 && (m.simulation_level & SIMULATION_LEVEL_DISABLED
								!= SIMULATION_LEVEL_DISABLED)
							{
								let adj_tiles = adjacent_tile_ids(adj, i, max_x, max_y);
								if let Some(gas) = all_mixtures.get(m.mix).unwrap().try_read() {
									for (_, loc) in adj_tiles {
										if let Some(turf) = turf_gases().get(&loc) {
											if turf.simulation_level & SIMULATION_LEVEL_DISABLED
												!= SIMULATION_LEVEL_DISABLED
											{
												if let Some(entry) = all_mixtures.get(turf.mix) {
													if let Some(mix) = entry.try_read() {
														if gas.temperature_compare(&mix)
															|| gas.compare_with(
																&mix,
																MINIMUM_MOLES_DELTA_TO_MOVE,
															) {
															return true;
														}
													} else {
														return false;
													}
												}
											}
										}
									}
									// Obviously planetary atmos needs love too.
									if let Some(planet_atmos_id) = m.planetary_atmos {
										if let Some(planet_atmos_entry) =
											planetary_atmos().get(&planet_atmos_id)
										{
											let planet_atmos = planet_atmos_entry.value();
											if gas.temperature_compare(&planet_atmos)
												|| gas.compare_with(
													&planet_atmos,
													MINIMUM_MOLES_DELTA_TO_MOVE,
												) {
												return true;
											}
										}
									}
								}
							}
							false
						})
						.filter_map(|(&i, m_v)| {
							// m_v is a SharedValue<TurfMixture>. Need to use get() on it to get the original.
							// It's dereferenced to copy it. m_v is Copy, so this is reasonably fast.
							// This is necessary because otherwise we're escaping a reference to the closure below.
							let m = *m_v.get();
							let adj = m.adjacency;
							/*
								We don't want to simulate space turfs or other unsimulated turfs. They're
								still valid for sharing to and from, they just shouldn't be considered
								for this particular step.
							*/
							if m.simulation_level > SIMULATION_LEVEL_NONE
								&& adj > 0 && (m.simulation_level & SIMULATION_LEVEL_DISABLED
								!= SIMULATION_LEVEL_DISABLED)
							{
								let adj_tiles = adjacent_tile_ids(adj, i, max_x, max_y);
								let mut adj_amount = 0;
								/*
									Getting write locks is potential danger zone,
									so we make sure we don't do that unless we
									absolutely need to. Saving is fast enough.
								*/
								let mut end_gas = GasMixture::from_vol(2500.0);
								let mut pressure_diffs: [(TurfID, f32); 6] = Default::default();
								/*
									The pressure here is negative
									because we're going to be adding it
									to the base turf's pressure later on.
									It's multiplied by the diffusion constant
									because it's not representing the total
									gas pressure difference but the force exerted
									due to the pressure gradient.
									Technically that's ρν², but, like, video games.
								*/
								for (j, loc) in adj_tiles {
									if let Some(turf) = turf_gases().get(&loc) {
										if turf.simulation_level & SIMULATION_LEVEL_DISABLED
											!= SIMULATION_LEVEL_DISABLED
										{
											if let Some(entry) = all_mixtures.get(turf.mix) {
												if let Some(mix) = entry.try_read() {
													end_gas.merge(&mix);
													adj_amount += 1;
													pressure_diffs[j as usize] = (
														loc,
														-mix.return_pressure()
															* GAS_DIFFUSION_CONSTANT,
													);
												} else {
													return None;
												}
											}
										}
									}
								}
								if let Some(planet_atmos_id) = m.planetary_atmos {
									if let Some(planet_atmos_entry) =
										planetary_atmos().get(&planet_atmos_id)
									{
										let planet_atmos = planet_atmos_entry.value();
										end_gas.merge(&planet_atmos);
										adj_amount += 1;
									}
								}
								/*
									This method of simulating diffusion
									diverges at coefficients that are
									larger than the inverse of the number
									of adjacent finite elements.
									As such, we must multiply it
									by a coefficient that is at most
									as big as this coefficient. The
									GAS_DIFFUSION_CONSTANT chosen here
									is 1/8, chosen both because it is
									smaller than 1/7 and because, in
									floats, 1/8 is exact and so are
									all multiples of it up to 1.
									(Technically up to 2,097,152,
									but I digress.)
								*/
								end_gas.multiply(GAS_DIFFUSION_CONSTANT);
								Some((i, m, end_gas, pressure_diffs, adj_amount))
							} else {
								None
							}
						})
						.collect::<Vec<_>>()
				})
				.flatten()
				.collect::<Vec<_>>();
			/*
				For the optimization-heads reading this: this is not an unnecessary collect().
				Saving all this to the turfs_to_save vector is, in fact, the reason
				that gases don't need an archive anymore--this *is* the archival step,
				simultaneously saving how the gases will change after the fact.
				In short: the above actually needs to finish before the below starts
				for consistency, so collect() is desired. This has been tested, by the way.
			*/
			let (low_pressure, high_pressure): (Vec<_>, Vec<_>) = turfs_to_save
				.par_iter()
				.filter_map(|(i, m, end_gas, mut pressure_diffs, adj_amount)| {
					if let Some(entry) = all_mixtures.get(m.mix) {
						let mut max_diff = 0.0f32;
						let moved_pressure = {
							let gas = entry.read();
							gas.return_pressure() * GAS_DIFFUSION_CONSTANT
						};
						for pressure_diff in pressure_diffs.iter_mut() {
							// pressure_diff.1 here was set to a negative above, so we just add.
							pressure_diff.1 += moved_pressure;
							max_diff = max_diff.max(pressure_diff.1.abs());
						}
						/*
							1.0 - GAS_DIFFUSION_CONSTANT * adj_amount is going to be
							precisely equal to the amount the surrounding tiles'
							end_gas have "taken" from this tile--
							they didn't actually take anything, just calculated
							how much would be. This is the "taking" step.
							Just to illustrate: say you have a turf with 3 neighbors.
							Each of those neighbors will have their end_gas added to by
							GAS_DIFFUSION_CONSTANT (at this writing, 0.125) times
							this gas. So, 1.0 - (0.125 * adj_amount) = 0.625--
							exactly the amount those gases "took" from this.
						*/
						{
							let gas: &mut GasMixture = &mut entry.write();
							gas.multiply(1.0 - (*adj_amount as f32 * GAS_DIFFUSION_CONSTANT));
							gas.merge(&end_gas);
						}
						/*
							If there is neither a major pressure difference
							nor are there any visible gases nor does it need
							to react, we're done outright. We don't need
							to do any more and we don't need to send the
							value to byond, so we don't. However, if we do...
						*/
						Some((*i, pressure_diffs, max_diff))
					} else {
						None
					}
				})
				.partition(|&(_, _, max_diff)| max_diff <= 5.0);
			if cur_count == 1 {
				let pressure_deltas_chunked = high_pressure.par_chunks(20).collect::<Vec<_>>();
				pressure_deltas_chunked
					.par_iter()
					.with_min_len(5)
					.for_each(|temp_value| {
						let sender = byond_callback_sender();
						let these_pressure_deltas = temp_value.iter().copied().collect::<Vec<_>>();
						let _ = sender.try_send(Box::new(move || {
							for &(turf_id, pressure_diffs, _) in
								these_pressure_deltas.iter().filter(|&(id, _, _)| *id != 0)
							{
								let turf = unsafe { Value::turf_by_id_unchecked(turf_id) };
								for &(id, diff) in pressure_diffs.iter() {
									if id != 0 {
										let enemy_tile = unsafe { Value::turf_by_id_unchecked(id) };
										if diff > 5.0 {
											turf.call(
												"consider_pressure_difference",
												&[&enemy_tile, &Value::from(diff)],
											)?;
										} else if diff < -5.0 {
											enemy_tile.call(
												"consider_pressure_difference",
												&[&turf.clone(), &Value::from(-diff)],
											)?;
										}
									}
								}
							}
							Ok(Value::null())
						}));
					});
			}
			high_pressure_turfs.extend(high_pressure.iter().map(|(i, _, _)| i));
			low_pressure_turfs.extend(low_pressure.iter().map(|(i, _, _)| i));
		});
		cur_count += 1;
	}
	(low_pressure_turfs, high_pressure_turfs)
}

fn excited_group_processing(
	max_x: i32,
	max_y: i32,
	pressure_goal: f32,
	low_pressure_turfs: BTreeSet<TurfID>,
) -> usize {
	let mut found_turfs: BTreeSet<TurfID> = BTreeSet::new();
	for &initial_turf in low_pressure_turfs.iter() {
		if found_turfs.contains(&initial_turf) {
			continue;
		}
		if let Some(initial_mix_ref) = turf_gases().get(&initial_turf) {
			let mut border_turfs: VecDeque<(TurfID, TurfMixture)> = VecDeque::with_capacity(40);
			let mut turfs: Vec<TurfMixture> = Vec::with_capacity(200);
			let mut min_pressure = initial_mix_ref.return_pressure();
			let mut max_pressure = min_pressure;
			let mut fully_mixed = GasMixture::new();
			border_turfs.push_back((initial_turf, *initial_mix_ref.value()));
			found_turfs.insert(initial_turf);
			GasMixtures::with_all_mixtures(|all_mixtures| {
				loop {
					if turfs.len() >= 2500 {
						break;
					}
					if let Some((i, turf)) = border_turfs.pop_front() {
						let adj_tiles = adjacent_tile_ids(turf.adjacency, i, max_x, max_y);
						let mix = all_mixtures.get(turf.mix).unwrap().read();
						let pressure = mix.return_pressure();
						let this_max = max_pressure.max(pressure);
						let this_min = min_pressure.min(pressure);
						if (this_max - this_min).abs() >= pressure_goal {
							continue;
						}
						min_pressure = this_min;
						max_pressure = this_max;
						turfs.push(turf);
						fully_mixed.merge(&mix);
						fully_mixed.volume += mix.volume;
						for (_, loc) in adj_tiles {
							if found_turfs.contains(&loc) {
								continue;
							}
							found_turfs.insert(loc);
							if let Some(border_mix) = turf_gases().get(&loc) {
								if border_mix.simulation_level & SIMULATION_LEVEL_DISABLED
									!= SIMULATION_LEVEL_DISABLED
								{
									border_turfs.push_back((loc, *border_mix));
								}
							}
						}
					} else {
						break;
					}
				}
				fully_mixed.multiply(1.0 / turfs.len() as f32);
				if !fully_mixed.is_corrupt() {
					turfs.par_iter().for_each(|turf| {
						let mut mix = all_mixtures.get(turf.mix).unwrap().write();
						mix.copy_from_mutable(&fully_mixed);
					});
				}
			});
		}
	}
	found_turfs.len()
}

static mut PLANET_RESET_TIMER: Option<Instant> = None;

fn post_process() {
	let should_check_planet_turfs = unsafe {
		if let Some(timer) = PLANET_RESET_TIMER.as_mut() {
			if timer.elapsed() > Duration::from_secs(5) {
				*timer = Instant::now();
				true
			} else {
				false
			}
		} else {
			PLANET_RESET_TIMER = Some(Instant::now());
			false
		}
	};
	let vis_vec = crate::gas::visibility_copies();
	let vis = vis_vec.as_slice();
	turf_gases().shards().par_iter().for_each(|shard| {
		let sender = byond_callback_sender();
		GasMixtures::with_all_mixtures(|all_mixtures| {
			let mut reacters = VecDeque::with_capacity(10);
			let mut visual_updaters = VecDeque::with_capacity(10);
			shard
				.read()
				.iter()
				.filter_map(|(&i, m_v)| {
					let m = m_v.get();
					if m.simulation_level > 0
						&& m.simulation_level & SIMULATION_LEVEL_DISABLED
							!= SIMULATION_LEVEL_DISABLED
					{
						if should_check_planet_turfs {
							if let Some(planet_atmos_id) = m.planetary_atmos {
								if let Some(planet_atmos_entry) =
									planetary_atmos().get(&planet_atmos_id)
								{
									let planet_atmos = planet_atmos_entry.value();
									if {
										if let Some(gas) =
											all_mixtures.get(m.mix).unwrap().try_read()
										{
											let comparison = gas.compare(planet_atmos);
											comparison < 1.0 && comparison > 0.0
										} else {
											false
										}
									} {
										if let Some(mut gas) =
											all_mixtures.get(m.mix).unwrap().try_write()
										{
											gas.copy_from_mutable(planet_atmos);
										}
									}
								}
							}
						}
						if let Some(gas) = all_mixtures.get(m.mix).unwrap().try_read() {
							let visibility = gas.visibility_hash(vis);
							let should_update_visuals = check_and_update_vis_hash(i, visibility);
							let reactable = gas.can_react();
							if should_update_visuals || reactable {
								return Some((i, should_update_visuals, reactable));
							}
						}
					}
					return None;
				})
				.for_each(|(i, should_update_visuals, reactable)| {
					if should_update_visuals {
						visual_updaters.push_back(i);
						if reacters.len() >= 10 {
							let copy = visual_updaters.drain(..).collect::<Vec<_>>();
							let _ = sender.try_send(Box::new(move || {
								for &i in copy.iter() {
									let turf = unsafe { Value::turf_by_id_unchecked(i) };
									turf.call("update_visuals", &[])?;
								}
								Ok(Value::null())
							}));
						}
					}
					if reactable {
						reacters.push_back(i);
						if reacters.len() >= 10 {
							let copy = reacters.drain(..).collect::<Vec<_>>();
							let _ = sender.try_send(Box::new(move || {
								for &i in copy.iter() {
									let turf = unsafe { Value::turf_by_id_unchecked(i) };
									turf.get(byond_string!("air"))?.call("react", &[&turf])?;
								}
								Ok(Value::null())
							}));
						}
					}
				});
			let _ = sender.try_send(Box::new(move || {
				for &i in reacters.iter() {
					let turf = unsafe { Value::turf_by_id_unchecked(i) };
					turf.get(byond_string!("air"))?.call("react", &[&turf])?;
				}
				for &i in visual_updaters.iter() {
					let turf = unsafe { Value::turf_by_id_unchecked(i) };
					turf.call("update_visuals", &[])?;
				}
				Ok(Value::null())
			}));
		});
	});
}

static HEAT_PROCESS_TIME: AtomicU64 = AtomicU64::new(1000000);

#[hook("/datum/controller/subsystem/air/proc/heat_process_time")]
fn _process_heat_time() {
	let tot = HEAT_PROCESS_TIME.load(Ordering::SeqCst);
	Ok(Value::from(
		Duration::new(tot / 1_000_000_000, (tot % 1_000_000_000) as u32).as_millis() as f32,
	))
}

static PROCESSING_HEAT: AtomicBool = AtomicBool::new(false);

// Expected function call: process_turf_heat()
// Returns: TRUE if thread not done, FALSE otherwise
#[hook("/datum/controller/subsystem/air/proc/process_turf_heat")]
fn _process_heat_hook() {
	/*
		Replacing LINDA's superconductivity system is this much more brute-force
		system--it shares heat between turfs and their neighbors,
		then receives and emits radiation to space, then shares
		between turfs and their gases. Since the latter requires a write lock,
		it's done after the previous step. This one doesn't care about
		consistency like the processing step does--this can run in full parallel.
	*/
	if PROCESSING_HEAT.compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
		== Ok(false)
	{
		/*
			Can't get a number from src in the thread, so we get it here.
			Have to get the time delta because the radiation
			is actually physics-based--the stefan boltzmann constant
			and radiation from space both have dimensions of second^-1 that
			need to be multiplied out to have any physical meaning.
			They also have dimensions of meter^-2, but I'm assuming
			turf tiles are 1 meter^2 anyway--the atmos subsystem
			does this in general, thus turf gas mixtures being 2.5 m^3.
		*/
		let time_delta = (src.get_number(byond_string!("wait")).map_err(|_| {
			runtime!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})? / 10.0) as f64;
		let max_x = auxtools::Value::world()
			.get_number(byond_string!("maxx"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})? as i32;
		let max_y = auxtools::Value::world()
			.get_number(byond_string!("maxy"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-number value as number {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})? as i32;
		rayon::spawn(move || {
			let start_time = Instant::now();
			let sender = byond_callback_sender();
			let emissivity_constant: f64 = STEFAN_BOLTZMANN_CONSTANT * time_delta;
			let radiation_from_space_tick: f64 = RADIATION_FROM_SPACE * time_delta;
			turf_temperatures()
				/*
					Same weird shard trick as above.
				*/
				.shards()
				.par_iter()
				.map(|shard| {
					shard
						.read()
						.iter()
						.filter_map(|(&i, t_v)| {
							let t = *t_v.get();
							let adj = t.adjacency;
							/*
								If it has no thermal conductivity or low thermal capacity,
								then it's not gonna interact, or at least shouldn't.
							*/
							if t.thermal_conductivity > 0.0 && t.heat_capacity > 300.0 && adj > 0 {
								let mut heat_delta = 0.0;
								let adj_tiles = adjacent_tile_ids(adj, i, max_x, max_y);
								let mut is_temp_delta_with_air = false;
								if let Some(m) = turf_gases().get(&i) {
									if m.simulation_level & SIMULATION_LEVEL_ANY > 0 {
										GasMixtures::with_all_mixtures(|all_mixtures| {
											if let Some(entry) = all_mixtures.get(m.mix) {
												if let Some(gas) = entry.try_read() {
													if (t.temperature - gas.get_temperature()).abs()
														> 1.0
													{
														is_temp_delta_with_air = true;
													}
												}
											}
										});
									}
								}
								for (_, loc) in adj_tiles {
									if let Some(other) = turf_temperatures().get(&loc) {
										heat_delta +=
											t.thermal_conductivity.min(other.thermal_conductivity)
												* (other.temperature - t.temperature) * (t
												.heat_capacity
												* other.heat_capacity
												/ (t.heat_capacity + other.heat_capacity));
										/*
											The horrible line above is essentially
											sharing between solids--making it the minimum of both
											conductivities makes this consistent, funnily enough.
										*/
									}
								}
								if t.adjacent_to_space {
									/*
										Straight up the standard blackbody radiation
										equation. All these are f64s because
										f32::MAX^4 < f64::MAX^(1/4), and t.temperature
										is ordinarily an f32, meaning that
										this will never go into infinities.
									*/
									let blackbody_radiation: f64 = (emissivity_constant
										* ((t.temperature as f64).powi(4)))
										- radiation_from_space_tick;
									heat_delta -= blackbody_radiation as f32;
								}
								let temp_delta = heat_delta / t.heat_capacity;
								if is_temp_delta_with_air || temp_delta.abs() > 0.1 {
									Some((i, t.temperature + temp_delta))
								} else {
									None
								}
							} else {
								None
							}
						})
						.collect::<Vec<_>>()
				})
				.flatten()
				.collect::<Vec<_>>() // for consistency, unfortunately
				.iter()
				.for_each(|&(i, new_temp)| {
					let t: &mut ThermalInfo = &mut turf_temperatures().get_mut(&i).unwrap();
					if let Some(m) = turf_gases().get(&i) {
						if m.simulation_level != SIMULATION_LEVEL_NONE
							&& m.simulation_level & SIMULATION_LEVEL_DISABLED
								!= SIMULATION_LEVEL_DISABLED
						{
							GasMixtures::with_all_mixtures(|all_mixtures| {
								if let Some(entry) = all_mixtures.get(m.mix) {
									let gas: &mut GasMixture = &mut entry.write();
									t.temperature = gas.temperature_share_non_gas(
										/*
											This value should be lower than the
											turf-to-turf conductivity for balance reasons
											as well as realism, otherwise fires will
											just sort of solve theirselves over time.
										*/
										t.thermal_conductivity * OPEN_HEAT_TRANSFER_COEFFICIENT,
										new_temp,
										t.heat_capacity,
									);
								}
							});
						} else {
							t.temperature = new_temp;
						}
					} else {
						t.temperature = new_temp;
					}
					if !t.temperature.is_normal() {
						t.temperature = 2.7
					}
					if t.heat_capacity > MINIMUM_TEMPERATURE_START_SUPERCONDUCTION
						&& t.temperature > t.heat_capacity
					{
						// not what heat capacity means but whatever
						let _ = sender.try_send(Box::new(move || {
							let turf = unsafe { Value::turf_by_id_unchecked(i) };
							turf.set(byond_string!("to_be_destroyed"), 1.0)?;
							Ok(Value::null())
						}));
					}
				});
			PROCESSING_HEAT.store(false, Ordering::SeqCst);
			//Alright, now how much time did that take?
			let bench = start_time.elapsed().as_nanos();
			let old_bench = HEAT_PROCESS_TIME.load(Ordering::SeqCst);
			// We display this as part of the MC atmospherics stuff.
			HEAT_PROCESS_TIME.store((old_bench * 3 + (bench * 7) as u64) / 10, Ordering::SeqCst);
		});
	}
	let arg_limit = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of arguments to heat processing: 0"))?
		.as_number()
		.map_err(|_| {
			runtime!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})?;
	Ok(Value::from(process_callbacks_for_millis(arg_limit as u64)))
}

#[shutdown]
fn reset_auxmos_processing() {
	PROCESSING_TURF_STEP.store(PROCESS_NOT_STARTED, Ordering::SeqCst);
	PROCESSING_HEAT.store(false, Ordering::SeqCst);
	HEAT_PROCESS_TIME.store(1000000, Ordering::SeqCst);
}
