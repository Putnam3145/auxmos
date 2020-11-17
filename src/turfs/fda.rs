use super::*;

use crate::GasMixtures;

use coarsetime::{Duration, Instant};

/*
A finite difference analysis diffusion engine.

Replaces LINDA, since the overhead of keeping active turfs isn't worth the performance gain.

Yeah, it's seriously fast enough to just operate on every turf now, go figure.
*/

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

const TURF_STEP_NOT_STARTED: u8 = 0;

const TURF_STEP_PROCESSING: u8 = 1;

const TURF_STEP_DONE: u8 = 2;

static PROCESSING_TURF_STEP: AtomicU8 = AtomicU8::new(TURF_STEP_NOT_STARTED);

static TURF_PROCESS_TIME: AtomicU64 = AtomicU64::new(1000000);

#[hook("/datum/controller/subsystem/air/proc/process_turfs_extools")]
fn _process_turf_hook() {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature (sleeping turfs).
		It can run in parallel, but doesn't yet. We'll see if it's required for performance reasons.
	*/
	// First we copy the gas list immutably, so we can be sure this is consistent.
	if PROCESSING_TURF_STEP.compare_and_swap(
		TURF_STEP_NOT_STARTED,
		TURF_STEP_PROCESSING,
		Ordering::SeqCst,
	) == TURF_STEP_NOT_STARTED
	{
		let cb = Callback::new(args.get(0).unwrap())?;
		rayon::spawn(move || {
			let start_time = Instant::now();
			let max_x = TurfGrid::max_x();
			let max_y = TurfGrid::max_y();
			let mut turfs_to_save: Vec<(usize, TurfMixture, GasMixture, [(u32, f32); 6])> =
				TURF_GASES
					.shards()
					.iter()
					.par_bridge()
					.map(|shard| {
						shard.read().iter().filter_map(|(&i, m_v)| {
							let m = *m_v.get();
							let adj = m.adjacency;
							if m.simulation_level >= SIMULATION_LEVEL_SIMULATE && adj > 0 {
								let gas = m.get_gas_copy();
								let adj_tiles = adjacent_tile_ids(adj, i, max_x, max_y);
								let mut should_share = false;
								GasMixtures::with_all_mixtures(|all_mixtures| {
									for loc in adj_tiles.iter() {
										if let Some(turf) = TURF_GASES.get(loc) {
											if let Ok(adj_gas) =
												all_mixtures.get(turf.mix).unwrap().read()
											{
												if gas
													.compare(&adj_gas, MINIMUM_MOLES_DELTA_TO_MOVE)
												{
													should_share = true;
													return;
												}
											}
										}
									}
								});
								if let Some(planet_atmos) = m.planetary_atmos {
									if gas.compare(
										PLANETARY_ATMOS.get(planet_atmos).unwrap().value(),
										0.01,
									) {
										should_share = true;
									}
								}
								if should_share {
									let mut end_gas = GasMixture::from_vol(2500.0);
									let mut pressure_diffs: [(u32, f32); 6] = Default::default();
									GasMixtures::with_all_mixtures(|all_mixtures| {
										let mut j = 0;
										for loc in adj_tiles.iter() {
											if let Some(turf) = TURF_GASES.get(loc) {
												if let Some(entry) = all_mixtures.get(turf.mix) {
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
									if let Some(planet_atmos) = m.planetary_atmos {
										end_gas.merge(
											PLANETARY_ATMOS.get(planet_atmos).unwrap().value(),
										);
									}
									end_gas.multiply(GAS_DIFFUSION_CONSTANT);
									Some((i, m, end_gas, pressure_diffs))
								} else {
									None
								}
							} else {
								None
							}
						}).collect::<Vec<_>>()
					})
					.flatten()
					.collect();
			turfs_to_save
				.par_iter_mut()
				.for_each(|(i, m, end_gas, pressure_diffs)| {
					let mut flags = 0;
					let adj_amount =
						m.adjacency.count_ones() + (m.planetary_atmos.is_some() as u32);
					/*
					Finally, we merge the end gas to the original,
					which multiplied so that it has the parts it shared to removed.
					*/

					GasMixtures::with_all_mixtures(|all_mixtures| {
						if let Some(entry) = all_mixtures.get(m.mix) {
							let gas: &mut GasMixture = &mut entry.write().unwrap();
							let moved_pressure = gas.return_pressure() * GAS_DIFFUSION_CONSTANT;
							gas.multiply(1.0 - (GAS_DIFFUSION_CONSTANT * adj_amount as f32));
							let mut pressure_diff_exists = false;
							for pressure_diff in pressure_diffs.iter_mut() {
								pressure_diff.1 += moved_pressure;
								pressure_diff_exists =
									pressure_diff_exists || pressure_diff.1.abs() > f32::EPSILON
							}
							gas.merge(&end_gas);
							if gas.is_visible() {
								flags |= 1;
							}
							if gas.can_react() {
								flags |= 2;
							}
							if pressure_diff_exists || flags > 0 {
								let turf_id = *i;
								let diffs_copy = *pressure_diffs;
								cb.invoke(move || {
									let turf = TurfGrid::turf_by_id(turf_id as u32);
									let true_pressure_diffs = List::new();
									for &(id, diff) in diffs_copy.iter() {
										if diff.abs() > f32::EPSILON {
											let sub_list = List::new();
											sub_list.append(&TurfGrid::turf_by_id(id));
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
			let bench = start_time.elapsed().as_nanos();
			PROCESSING_TURF_STEP.store(TURF_STEP_DONE, Ordering::SeqCst);
			let old_bench = TURF_PROCESS_TIME.load(Ordering::Relaxed);
			TURF_PROCESS_TIME.store((old_bench * 3 + bench * 7) / 10, Ordering::Relaxed);
		});
	}
	if PROCESSING_TURF_STEP.compare_and_swap(
		TURF_STEP_DONE,
		TURF_STEP_NOT_STARTED,
		Ordering::Relaxed,
	) == TURF_STEP_DONE
	{
		Ok(Value::from(1.0))
	} else {
		Ok(Value::from(0.0))
	}
}

#[hook("/datum/controller/subsystem/air/proc/turf_process_time")]
fn _process_turf_time() {
	let tot = TURF_PROCESS_TIME.load(Ordering::Relaxed);
	Ok(Value::from(
		(Duration::new(tot / 1_000_000_000, (tot % 1_000_000_000) as u32).as_f64() * 1e3) as f32,
	))
}

static PROCESSING_HEAT: AtomicBool = AtomicBool::new(false);

#[hook("/datum/controller/subsystem/air/proc/process_turf_heat")]
fn _process_heat_hook() {
	if PROCESSING_HEAT.compare_and_swap(false, true, Ordering::SeqCst) == false {
		let time_delta = (src.get_number("wait")? / 10.0) as f64;
		let emissivity_constant: f64 = STEFAN_BOLTZMANN_CONSTANT * time_delta;
		let radiation_from_space_tick: f64 = RADIATION_FROM_SPACE * time_delta;
		// Unlike non-heat diffusion, this doesn't need to be consistent at all--
		// this is the only thing in the entire codebase that should be
		// modifying turf temperatures.
		// So, we can just shove the whole thing into a different thread.
		rayon::spawn(move || {
			let max_x = TurfGrid::max_x();
			let max_y = TurfGrid::max_y();
			let post_temps: Vec<(usize, f32)> = TURF_TEMPERATURES
				.iter()
				.filter_map(|e| {
					let (i, t) = (*e.key(), *e.value());
					let adj = t.adjacency;
					if t.thermal_conductivity > 0.0 && t.heat_capacity > 0.0 && adj > 0 {
						let mut heat_delta = 0.0;
						let adj_tiles = adjacent_tile_ids(adj, i, max_x, max_y);
						for loc in adj_tiles.iter() {
							if let Some(other) = TURF_TEMPERATURES.get(loc) {
								heat_delta +=
									t.thermal_conductivity.min(other.thermal_conductivity)
										* (other.temperature - t.temperature) * (t.heat_capacity
										* other.heat_capacity
										/ (t.heat_capacity + other.heat_capacity));
							}
						}
						let cur_heat = t.temperature * t.heat_capacity;
						if t.adjacent_to_space {
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
				.collect();
			post_temps.par_iter().for_each(|&(i, new_temp)| {
				let t: &mut ThermalInfo = &mut TURF_TEMPERATURES.get_mut(&i).unwrap();
				if let Some(m) = TURF_GASES.get(&i) {
					GasMixtures::with_all_mixtures(|all_mixtures| {
						if let Some(entry) = all_mixtures.get(m.mix) {
							let gas: &mut GasMixture = &mut entry.write().unwrap();
							t.temperature = gas.temperature_share_non_gas(
								t.thermal_conductivity * OPEN_HEAT_TRANSFER_COEFFICIENT,
								new_temp,
								t.heat_capacity,
							);
						}
					});
				} else {
					t.temperature = new_temp;
				}
			});
			PROCESSING_HEAT.store(false, Ordering::SeqCst);
		});
		Ok(Value::from(true))
	} else {
		Ok(Value::from(false))
	}
}
