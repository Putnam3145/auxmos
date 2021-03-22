use super::*;

use crate::GasMixtures;

use std::time::{Duration, Instant};

use auxcallback::{callback_sender_by_id_insert, process_callbacks_for_millis};

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

const PROCESS_NOT_STARTED: u8 = 0;

const PROCESS_PROCESSING: u8 = 1;

const PROCESS_DONE: u8 = 2;

static PROCESSING_TURF_STEP: AtomicU8 = AtomicU8::new(PROCESS_NOT_STARTED);

static TURF_PROCESS_TIME: AtomicU64 = AtomicU64::new(1000000);

static HEAT_PROCESS_TIME: AtomicU64 = AtomicU64::new(1000000);

static SUBSYSTEM_FIRE_COUNT: AtomicU32 = AtomicU32::new(0);

lazy_static! {
	// Speeds up excited group processing.
	static ref LOW_PRESSURE_TURFS: (
		flume::Sender<TurfID>,
		flume::Receiver<TurfID>
	) = flume::unbounded();
}

// Returns: TRUE if not done, FALSE if done
#[hook("/datum/controller/subsystem/air/proc/process_turfs_auxtools")]
fn _process_turf_hook() {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature,
		sleeping turfs, which is why I've renamed it to FDA. It's FDA and not FDM mostly as a way to
		echo FEA, the original SS13 atmos system.
	*/
	// Don't want to start it while there's already a thread running, so we only start it if it hasn't been started.
	SUBSYSTEM_FIRE_COUNT.store(src.get_number("times_fired")? as u32, Ordering::SeqCst);
	let resumed = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of arguments to turf processing: 0"))?
		.as_number()?
		== 1.0;
	if !resumed
		&& PROCESSING_TURF_STEP.load(Ordering::SeqCst) == PROCESS_NOT_STARTED
		&& !PROCESSING_HEAT.load(Ordering::SeqCst)
	{
		let fda_max_steps = src.get_number("share_max_steps").unwrap_or_else(|_| 1.0) as i32;
		let fda_pressure_goal = src
			.get_number("share_pressure_diff_to_stop")
			.unwrap_or_else(|_| 101.325);
		let max_x = ctx.get_world().get_number("maxx")? as i32;
		let max_y = ctx.get_world().get_number("maxy")? as i32;
		rayon::spawn(move || {
			PROCESSING_TURF_STEP.store(PROCESS_PROCESSING, Ordering::SeqCst);
			let sender = callback_sender_by_id_insert(SSAIR_NAME.to_string());
			let start_time = Instant::now();
			let high_pressure_sender = HIGH_PRESSURE_TURFS.0.clone();
			let low_pressure_sender = LOW_PRESSURE_TURFS.0.clone();
			let initial_fire_count = SUBSYSTEM_FIRE_COUNT.load(Ordering::SeqCst);
			let mut cur_count = 1;
			GasMixtures::with_all_mixtures(|all_mixtures| {
				loop {
					if cur_count > fda_max_steps {
						break;
					}
					let turfs_to_save =
						TURF_GASES
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
							.iter()
							.par_bridge()
							.map(|shard| {
								shard
									.read()
									.iter()
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
											&& adj > 0 && (m.simulation_level
											& SIMULATION_LEVEL_DISABLED
											!= SIMULATION_LEVEL_DISABLED)
										{
											let adj_tiles = adjacent_tile_ids(adj, i, max_x, max_y);
											if let Some(gas) =
												all_mixtures.get(m.mix).unwrap().try_read()
											{
												/*
													Getting write locks is potential danger zone,
													so we make sure we don't do that unless we
													absolutely need to. Saving is fast enough.
												*/
												let mut end_gas = GasMixture::from_vol(2500.0);
												let mut pressure_diffs: [(TurfID, f32); 6] =
													Default::default();
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
												let mut should_share = false;
												for (j, loc) in adj_tiles {
													if let Some(turf) = TURF_GASES.get(&loc) {
														if turf.simulation_level
															& SIMULATION_LEVEL_DISABLED
															!= SIMULATION_LEVEL_DISABLED
														{
															if let Some(entry) =
																all_mixtures.get(turf.mix)
															{
																if let Some(mix) = entry.try_read()
																{
																	end_gas.merge(&mix);
																	pressure_diffs[j as usize] = (
																	loc,
																	-mix.return_pressure()
																		* GAS_DIFFUSION_CONSTANT,
																	);
																	if !should_share && gas.compare(&mix,MINIMUM_MOLES_DELTA_TO_MOVE) {
																		should_share = true;
																	}
																} else {
																	return None;
																}
															}
														}
													}
												}
												// Obviously planetary atmos needs love too.
												if let Some(planet_atmos_id) = m.planetary_atmos {
													if let Some(planet_atmos_entry) =
														PLANETARY_ATMOS.get(planet_atmos_id)
													{
														let planet_atmos =
															planet_atmos_entry.value();
														if should_share {
															end_gas.merge(planet_atmos);
														} else {
															if gas.compare(
																&planet_atmos,
																MINIMUM_MOLES_DELTA_TO_MOVE,
															) {
																end_gas.merge(planet_atmos);
																should_share = true;
															}
														}
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
												if should_share {
													end_gas.multiply(GAS_DIFFUSION_CONSTANT);
													Some((i, m, end_gas, pressure_diffs))
												} else {
													None
												}
											} else {
												None
											}
										} else {
											None
										}
									})
									.collect::<Vec<_>>()
							})
							.flatten();
					/*
						For the optimization-heads reading this: this is not an unnecessary collect().
						Saving all this to the turfs_to_save vector is, in fact, the reason
						that gases don't need an archive anymore--this *is* the archival step,
						simultaneously saving how the gases will change after the fact.
						In short: the above actually needs to finish before the below starts
						for consistency, so collect() is desired. This has been tested, by the way.
					*/
					let should_break = turfs_to_save
						.map(|(i, m, end_gas, mut pressure_diffs)| {
							let adj_amount = (m.adjacency.count_ones()
								+ (m.planetary_atmos.is_some() as u32)) as f32;
							let mut this_high_pressure = false;
							if let Some(entry) = all_mixtures.get(m.mix) {
								let mut pressure_diff_exists = false;
								let mut max_diff = 0.0f32;
								let moved_pressure = {
									let gas = entry.read();
									gas.return_pressure() * GAS_DIFFUSION_CONSTANT
								};
								for pressure_diff in pressure_diffs.iter_mut() {
									// pressure_diff.1 here was set to a negative above, so we just add.
									pressure_diff.1 += moved_pressure;
									max_diff = max_diff.max(pressure_diff.1);
									// See the explanation below.
									pressure_diff_exists =
										pressure_diff_exists || pressure_diff.1.abs() > 0.5;
									this_high_pressure = this_high_pressure
										|| pressure_diff.1.abs() > fda_pressure_goal;
								}
								if max_diff.abs() > 0.25 {
									let _ = high_pressure_sender.try_send(i);
								} else {
									let _ = low_pressure_sender.try_send(i);
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
									gas.multiply(1.0 - (GAS_DIFFUSION_CONSTANT * adj_amount));
									gas.merge(&end_gas);
								}
								/*
									If there is neither a major pressure difference
									nor are there any visible gases nor does it need
									to react, we're done outright. We don't need
									to do any more and we don't need to send the
									value to byond, so we don't. However, if we do...
								*/
								if pressure_diff_exists {
									let turf_id = i;
									let diffs_copy = pressure_diffs;
									sender
										.try_send(Box::new(move |_| {
											if turf_id == 0 {
												return Ok(Value::null());
											}
											let turf =
												unsafe { Value::turf_by_id_unchecked(turf_id) };
											for &(id, diff) in diffs_copy.iter() {
												if id != 0 {
													let enemy_tile =
														unsafe { Value::turf_by_id_unchecked(id) };
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
											Ok(Value::null())
										}))
										.unwrap();
								}
							}
							this_high_pressure
						})
						.reduce(|| false, |a, b| a || b);
					if should_break
						|| SUBSYSTEM_FIRE_COUNT.load(Ordering::SeqCst) != initial_fire_count
					{
						return;
					}
					cur_count += 1;
				}
			});
			//Alright, now how much time did that take?
			let bench = start_time.elapsed().as_nanos();
			PROCESSING_TURF_STEP.store(PROCESS_DONE, Ordering::SeqCst);
			let old_bench = TURF_PROCESS_TIME.load(Ordering::SeqCst);
			// We display this as part of the MC atmospherics stuff.
			TURF_PROCESS_TIME.store((old_bench * 3 + (bench * 7) as u64) / 10, Ordering::SeqCst);
		});
	}
	Ok(Value::from(false))
}

#[hook("/datum/controller/subsystem/air/proc/finish_turf_processing_auxtools")]
fn _finish_process_turfs() {
	let arg_limit = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of arguments to turf finishing: 0"))?
		.as_number()?;
	process_callbacks_for_millis(ctx, SSAIR_NAME.to_string(), arg_limit as u64);
	// If PROCESSING_TURF_STEP is done, we're done, and we should set it to NOT_STARTED while we're at it.
	Ok(Value::from(
		PROCESSING_TURF_STEP.compare_exchange(
			PROCESS_DONE,
			PROCESS_NOT_STARTED,
			Ordering::SeqCst,
			Ordering::Relaxed,
		) == Err(PROCESS_PROCESSING),
	))
}

#[hook("/datum/controller/subsystem/air/proc/turf_process_time")]
fn _process_turf_time() {
	let tot = TURF_PROCESS_TIME.load(Ordering::SeqCst);
	Ok(Value::from(
		Duration::new(tot / 1_000_000_000, (tot % 1_000_000_000) as u32).as_millis() as f32,
	))
}

#[hook("/datum/controller/subsystem/air/proc/thread_running")]
fn _thread_running_hook() {
	Ok(Value::from(
		PROCESSING_TURF_STEP.load(Ordering::Relaxed) == PROCESS_PROCESSING,
	))
}

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
		let time_delta = (src.get_number("wait")? / 10.0) as f64;
		let max_x = ctx.get_world().get_number("maxx")? as i32;
		let max_y = ctx.get_world().get_number("maxy")? as i32;
		rayon::spawn(move || {
			let start_time = Instant::now();
			let sender = callback_sender_by_id_insert(SSAIR_NAME.to_string());
			let emissivity_constant: f64 = STEFAN_BOLTZMANN_CONSTANT * time_delta;
			let radiation_from_space_tick: f64 = RADIATION_FROM_SPACE * time_delta;
			TURF_TEMPERATURES
				/*
					Same weird shard trick as above.
				*/
				.shards()
				.iter()
				.par_bridge()
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
								if let Some(m) = TURF_GASES.get(&i) {
									if m.simulation_level != SIMULATION_LEVEL_NONE
										&& m.simulation_level & SIMULATION_LEVEL_DISABLED
											!= SIMULATION_LEVEL_DISABLED
									{
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
									if let Some(other) = TURF_TEMPERATURES.get(&loc) {
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
				.flatten_iter()
				.for_each(|(i, new_temp)| {
					let t: &mut ThermalInfo = &mut TURF_TEMPERATURES.get_mut(&i).unwrap();
					if let Some(m) = TURF_GASES.get(&i) {
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
						let _ = sender.try_send(Box::new(move |_| {
							let turf = unsafe { Value::turf_by_id_unchecked(i) };
							turf.set("to_be_destroyed", 1.0);
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
		.as_number()?;
	Ok(Value::from(process_callbacks_for_millis(
		ctx,
		SSAIR_NAME.to_string(),
		arg_limit as u64,
	)))
}

static POST_PROCESS_STEP: AtomicU8 = AtomicU8::new(PROCESS_NOT_STARTED);

#[hook("/datum/controller/subsystem/air/proc/post_process_turfs_auxtools")]
fn _post_process_turfs() {
	let resumed = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of arguments to turf post processing: 0"))?
		.as_number()?
		== 1.0;
	let arg_limit = args
		.get(1)
		.ok_or_else(|| runtime!("Wrong number of arguments to turf post processing: 1"))?
		.as_number()?;
	if !resumed && POST_PROCESS_STEP.load(Ordering::SeqCst) == PROCESS_NOT_STARTED {
		rayon::spawn(move || {
			POST_PROCESS_STEP.store(PROCESS_PROCESSING, Ordering::SeqCst);
			TURF_GASES.shards().iter().par_bridge().for_each(|shard| {
				let sender = callback_sender_by_id_insert(SSAIR_NAME.to_string());
				GasMixtures::with_all_mixtures(|all_mixtures| {
					shard.write().iter_mut().for_each(|(&i, m_v)| {
						let m = m_v.get_mut();
						if m.simulation_level > SIMULATION_LEVEL_NONE
							&& (m.simulation_level & SIMULATION_LEVEL_DISABLED
								!= SIMULATION_LEVEL_DISABLED)
						{
							if let Some(gas) = all_mixtures.get(m.mix).unwrap().try_read() {
								let visibility = gas.visibility_hash();
								let should_update_visuals = m.vis_hash != visibility;
								m.vis_hash = visibility;
								let reactable = gas.can_react();
								if should_update_visuals || reactable {
									let _ = sender.try_send(Box::new(move |_| {
										let turf = unsafe { Value::turf_by_id_unchecked(i) };
										if reactable {
											turf.get(byond_string!("air"))?
												.call("react", &[&turf])?;
										}
										if should_update_visuals {
											turf.call("update_visuals", &[&Value::null()])?;
										}
										Ok(Value::null())
									}));
								}
							}
						}
					});
				});
			});
			POST_PROCESS_STEP.store(PROCESS_DONE, Ordering::SeqCst);
		});
	}
	Ok(Value::from(
		process_callbacks_for_millis(ctx, SSAIR_NAME.to_string(), arg_limit as u64)
			|| POST_PROCESS_STEP.compare_exchange(
				PROCESS_DONE,
				PROCESS_NOT_STARTED,
				Ordering::SeqCst,
				Ordering::Relaxed,
			) == Err(PROCESS_PROCESSING),
	))
}

static EXCITED_GROUP_STEP: AtomicU8 = AtomicU8::new(PROCESS_NOT_STARTED);

/*
	Flood fills every contiguous region it can find, equalizing
	the entire region if it doesn't find a pressure delta of
	more than 1 kilopascal.
*/
#[hook("/datum/controller/subsystem/air/proc/process_excited_groups_auxtools")]
fn process_excited_groups() {
	let resumed = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of arguments to excited group processing: 0"))?
		.as_number()?
		== 1.0;
	if !resumed && EXCITED_GROUP_STEP.load(Ordering::SeqCst) == PROCESS_NOT_STARTED {
		let max_x = ctx.get_world().get_number("maxx")? as i32;
		let max_y = ctx.get_world().get_number("maxy")? as i32;
		rayon::spawn(move || {
			use std::collections::{BTreeSet, VecDeque};
			EXCITED_GROUP_STEP.store(PROCESS_PROCESSING, Ordering::SeqCst);
			let mut found_turfs: BTreeSet<TurfID> = BTreeSet::new();
			let low_pressure_receiver = LOW_PRESSURE_TURFS.1.clone();
			for initial_turf in low_pressure_receiver.try_iter() {
				if found_turfs.contains(&initial_turf) {
					continue;
				}
				if let Some(initial_mix_ref) = TURF_GASES.get(&initial_turf) {
					let mut border_turfs: VecDeque<(TurfID, TurfMixture)> =
						VecDeque::with_capacity(40);
					let mut turfs: Vec<TurfMixture> = Vec::with_capacity(200);
					let mut min_pressure = initial_mix_ref.return_pressure();
					let mut max_pressure = min_pressure;
					let mut fully_mixed = GasMixture::from_vol(2500.0);
					border_turfs.push_back((initial_turf, *initial_mix_ref.value()));
					found_turfs.insert(initial_turf);
					GasMixtures::with_all_mixtures(|all_mixtures| {
						while !border_turfs.is_empty() && turfs.len() < 400 {
							let (i, turf) = border_turfs.pop_front().unwrap();
							turfs.push(turf);
							let adj_tiles = adjacent_tile_ids(turf.adjacency, i, max_x, max_y);
							let mix = all_mixtures.get(turf.mix).unwrap().read();
							let pressure = mix.return_pressure();
							min_pressure = min_pressure.min(pressure);
							max_pressure = max_pressure.max(pressure);
							if (max_pressure - min_pressure).abs() >= 1.0 || min_pressure <= 1.0 {
								return;
							}
							fully_mixed.merge(&mix);
							for (_, loc) in adj_tiles {
								if found_turfs.contains(&loc) {
									continue;
								}
								found_turfs.insert(loc);
								if let Some(border_mix) = TURF_GASES.get(&loc) {
									if border_mix.simulation_level == SIMULATION_LEVEL_ALL {
										border_turfs.push_back((loc, *border_mix));
									}
								}
							}
						}
						if max_pressure - min_pressure < 1.0 {
							fully_mixed.multiply((turfs.len() as f64).recip() as f32);
							for turf in turfs.iter() {
								let mut mix = all_mixtures.get(turf.mix).unwrap().write();
								mix.copy_from_mutable(&fully_mixed);
							}
						}
					});
				}
			}
			EXCITED_GROUP_STEP.store(PROCESS_DONE, Ordering::SeqCst);
		});
	}
	let arg_limit = args
		.get(1)
		.ok_or_else(|| runtime!("Wrong number of arguments to excited group processing: 1"))?
		.as_number()?;
	process_callbacks_for_millis(ctx, SSAIR_NAME.to_string(), arg_limit as u64);
	Ok(Value::from(
		EXCITED_GROUP_STEP.compare_exchange(
			PROCESS_DONE,
			PROCESS_NOT_STARTED,
			Ordering::SeqCst,
			Ordering::Relaxed,
		) == Err(PROCESS_PROCESSING),
	))
}
