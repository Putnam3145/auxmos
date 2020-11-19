use super::*;

use crate::GasMixtures;

use coarsetime::{Duration, Instant};

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

const TURF_STEP_NOT_STARTED: u8 = 0;

const TURF_STEP_PROCESSING: u8 = 1;

const TURF_STEP_DONE: u8 = 2;

static PROCESSING_TURF_STEP: AtomicU8 = AtomicU8::new(TURF_STEP_NOT_STARTED);

static TURF_PROCESS_TIME: AtomicU64 = AtomicU64::new(1000000);

/*
post_process_turf is the following function:
/proc/post_process_turf(flags,turf/open/T,list/tiles_with_diffs)
	if(!isopenturf(T))
		return
	if(flags & 2)
		T.air.react()
	if(flags & 1)
		T.update_visuals()
	for(var/list/pair in tiles_with_diffs)
		var/turf/open/enemy_tile = pair[1]
		if(istype(enemy_tile))
			var/difference = pair[2]
			if(difference > 0)
				T.consider_pressure_difference(enemy_tile, difference)
			else
				enemy_tile.consider_pressure_difference(T, -difference)
*/

// Expected function call: process_turfs_extools(CALLBACK(GLOBAL_PROC,/proc/post_process_turf))
// Returns: TRUE if not done, FALSE if done
#[hook("/datum/controller/subsystem/air/proc/process_turfs_extools")]
fn _process_turf_hook() {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature,
		sleeping turfs, which is why I've renamed it to FDA. It's FDA and not FDM mostly as a way to
		echo FEA, the original SS13 atmos system.
	*/
	// Don't want to start it while there's already a thread running, so we only start it if it hasn't been started.
	if PROCESSING_TURF_STEP.load(Ordering::SeqCst) == TURF_STEP_NOT_STARTED {
		// The callback does the in-byond reaction, updating visuals, pressure diffs.
		// It's run ASAP by the Auxtools callback subsystem.
		let cb = Callback::new(args.get(0).unwrap())?;
		let max_x = ctx.get_world().get_number("maxx")? as i32;
		let max_y = ctx.get_world().get_number("maxy")? as i32;
		rayon::spawn(move || {
			PROCESSING_TURF_STEP.store(TURF_STEP_PROCESSING, Ordering::SeqCst);
			let start_time = Instant::now();
			let high_pressure_sender = HIGH_PRESSURE_TURFS.0.clone();
			let mut turfs_to_save: Vec<(usize, TurfMixture, GasMixture, [(u32, f32); 6])> =
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
								if m.simulation_level >= SIMULATION_LEVEL_SIMULATE && adj > 0 {
									let adj_tiles = adjacent_tile_ids(adj, i, max_x, max_y);
									/*
										Getting write locks is potential danger zone,
										so we make sure we don't do that unless we
										absolutely need to. Saving is fast enough.
									*/
									let mut should_share = false;
									GasMixtures::with_all_mixtures(|all_mixtures| {
										let gas = all_mixtures.get(m.mix).unwrap().read().unwrap();
										for (_, loc) in adj_tiles.iter() {
											if let Some(turf) = TURF_GASES.get(loc) {
												if let Ok(adj_gas) =
													all_mixtures.get(turf.mix).unwrap().read()
												{
													if gas.compare(
														&adj_gas,
														MINIMUM_MOLES_DELTA_TO_MOVE,
													) {
														should_share = true;
														return;
													}
												}
											}
										}
										if let Some(planet_atmos) = m.planetary_atmos {
											if gas.compare(
												PLANETARY_ATMOS.get(planet_atmos).unwrap().value(),
												0.01,
											) {
												should_share = true;
											}
										}
									});
									if should_share {
										let mut end_gas = GasMixture::from_vol(2500.0);
										let mut pressure_diffs: [(u32, f32); 6] =
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
										GasMixtures::with_all_mixtures(|all_mixtures| {
											let mut j = 0;
											for (_,loc) in adj_tiles.iter() {
												if let Some(turf) = TURF_GASES.get(loc) {
													if let Some(entry) = all_mixtures.get(turf.mix)
													{
														if let Ok(mix) = entry.read() {
															end_gas.merge(&mix);
															pressure_diffs[j] = (
																*loc as u32,
																-mix.return_pressure()
																	* GAS_DIFFUSION_CONSTANT,
															);
														}
													}
												}
												j += 1;
											}
										});
										// Obviously planetary atmos needs love too.
										if let Some(planet_atmos) = m.planetary_atmos {
											end_gas.merge(
												PLANETARY_ATMOS.get(planet_atmos).unwrap().value(),
											);
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
										Some((i, m, end_gas, pressure_diffs))
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
					.collect();
			/*
				For the optimization-heads reading this: this is not an unnecessary collect().
				Saving all this to the turfs_to_save vector is, in fact, the reason
				that gases don't need an archive anymore--this *is* the archival step,
				simultaneously saving how the gases will change after the fact.
				In short: the above actually needs to finish before the below starts
				for consistency, so collect() is desired.
			*/
			turfs_to_save
				.par_iter_mut()
				.for_each(|(i, m, end_gas, pressure_diffs)| {
					let mut flags = 0;
					let adj_amount =
						(m.adjacency.count_ones() + (m.planetary_atmos.is_some() as u32)) as f32;
					GasMixtures::with_all_mixtures(|all_mixtures| {
						if let Some(entry) = all_mixtures.get(m.mix) {
							let gas: &mut GasMixture = &mut entry.write().unwrap();
							let moved_pressure = gas.return_pressure() * GAS_DIFFUSION_CONSTANT;
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
							gas.multiply(1.0 - (GAS_DIFFUSION_CONSTANT * adj_amount));
							let mut pressure_diff_exists = false;
							let mut max_diff = 0.0f32;
							for pressure_diff in pressure_diffs.iter_mut() {
								// pressure_diff.1 here was set to a negative above, so we just add.
								pressure_diff.1 += moved_pressure;
								max_diff = max_diff.max(pressure_diff.1);
								// See the explanation below.
								pressure_diff_exists =
									pressure_diff_exists || pressure_diff.1.abs() > f32::EPSILON
							}
							if max_diff > 1.0 {
								let _ = high_pressure_sender.send(*i);
							}
							gas.merge(&end_gas);
							/*
								And we're done moving gases for this turf.
								We use these bitflags because reacting and
								visibility are, well, incredibly slow, on the byond end.
								We can check if the gas needs either to be done
								in the Rust side fantastically quickly, though, so
								we do, as a way to keep the byond end from
								causing too much trouble.
							*/
							if gas.is_visible() {
								flags |= 1;
							}
							if gas.can_react() {
								flags |= 2;
							}
							/*
								If there is neither a major pressure difference
								nor are there any visible gases nor does it need
								to react, we're done outright. We don't need
								to do any more and we don't need to send the
								value to byond, so we don't. However, if we do...
							*/
							if pressure_diff_exists || flags > 0 {
								let turf_id = *i;
								let diffs_copy = *pressure_diffs;
								cb.invoke(move || {
									let turf =
										unsafe { Value::turf_by_id_unchecked(turf_id as u32) };
									let true_pressure_diffs = List::new();
									for &(id, diff) in diffs_copy.iter() {
										if diff.abs() > f32::EPSILON {
											let sub_list = List::new();
											sub_list.append(&unsafe {
												Value::turf_by_id_unchecked(id)
											});
											sub_list.append(diff);
											true_pressure_diffs.append(&Value::from(sub_list));
										}
									}
									vec![
										Value::from(flags as f32),
										turf,
										Value::from(true_pressure_diffs),
									]
								});
							}
						}
					});
				});
			//Alright, now how much time did that take?
			let bench = start_time.elapsed().as_nanos();
			PROCESSING_TURF_STEP.store(TURF_STEP_DONE, Ordering::SeqCst);
			let old_bench = TURF_PROCESS_TIME.load(Ordering::Relaxed);
			// We display this as part of the MC atmospherics stuff.
			TURF_PROCESS_TIME.store((old_bench * 3 + bench * 7) / 10, Ordering::Relaxed);
		});
	}
	// If PROCESSING_TURF_STEP is done, we're done, and we should set it to NOT_STARTED while we're at it.
	Ok(Value::from(
		PROCESSING_TURF_STEP.compare_and_swap(
			TURF_STEP_DONE,
			TURF_STEP_NOT_STARTED,
			Ordering::Relaxed,
		) == TURF_STEP_DONE,
	))
}

#[hook("/datum/controller/subsystem/air/proc/turf_process_time")]
fn _process_turf_time() {
	let tot = TURF_PROCESS_TIME.load(Ordering::Relaxed);
	Ok(Value::from(
		(Duration::new(tot / 1_000_000_000, (tot % 1_000_000_000) as u32).as_f64() * 1e3) as f32,
	))
}

static PROCESSING_HEAT: AtomicBool = AtomicBool::new(false);

/*
heat_post_process is the following function:
/proc/heat_post_process(turf/T,new_temp)
	T.temperature = new_temp
	T.temperature_expose()
*/

// Expected function call: process_turf_heat(CALLBACK(GLOBAL_PROC,/proc/heat_post_process))
// Returns: TRUE if started the thread, FALSE otherwise
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
	if PROCESSING_HEAT.compare_and_swap(false, true, Ordering::SeqCst) == false {
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
		/*
			Similarly, can't get args in a thread, so we get the callback here.
			The callback is of the form:
			/proc/heat_post_process(turf/T,new_temp)
				T.temperature = new_temp
				T.temperature_expose()
		*/
		let cb = Callback::new(args.get(0).unwrap())?;
		let max_x = ctx.get_world().get_number("maxx")? as i32;
		let max_y = ctx.get_world().get_number("maxy")? as i32;
		rayon::spawn(move || {
			let emissivity_constant: f64 = STEFAN_BOLTZMANN_CONSTANT * time_delta;
			let radiation_from_space_tick: f64 = RADIATION_FROM_SPACE * time_delta;
			let post_temps: Vec<(usize, f32)> = TURF_TEMPERATURES
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
								If it has no thermal conductivity or no thermal capacity,
								then it's not gonna interact, or at least shouldn't.
							*/
							if t.thermal_conductivity > 0.0 && t.heat_capacity > 0.0 && adj > 0 {
								let mut heat_delta = 0.0;
								let adj_tiles = adjacent_tile_ids(adj, i, max_x, max_y);
								for (_, loc) in adj_tiles.iter() {
									if let Some(other) = TURF_TEMPERATURES.get(loc) {
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
								let cur_heat = t.temperature * t.heat_capacity;
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
								Some((i, (cur_heat + heat_delta) / t.heat_capacity))
							} else {
								None
							}
						})
						.collect::<Vec<_>>()
				})
				.flatten()
				.collect();
			post_temps.par_iter().for_each(|&(i, new_temp)| {
				let t: &mut ThermalInfo = &mut TURF_TEMPERATURES.get_mut(&i).unwrap();
				let original_temp = t.temperature;
				if let Some(m) = TURF_GASES.get(&i) {
					GasMixtures::with_all_mixtures(|all_mixtures| {
						if let Some(entry) = all_mixtures.get(m.mix) {
							let gas: &mut GasMixture = &mut entry.write().unwrap();
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
				// Temp diffs of less than 0.05 are meaningless and this callback, while not slow,
				// is still going to lag the hell out of the server if run for every turf
				if (original_temp - t.temperature).abs() > 0.05 {
					cb.invoke(move || {
						vec![
							unsafe { Value::turf_by_id_unchecked(i as u32) },
							Value::from(new_temp),
						]
					});
				}
			});
			PROCESSING_HEAT.store(false, Ordering::SeqCst);
		});
		Ok(Value::from(true))
	} else {
		Ok(Value::from(false))
	}
}
