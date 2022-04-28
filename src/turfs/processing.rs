use std::collections::{BTreeSet, VecDeque};

use auxtools::*;

use super::*;

use crate::GasArena;

use std::time::{Duration, Instant};

use auxcallback::{byond_callback_sender, process_callbacks_for_millis};

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use parking_lot::{Once, RwLock};

use crate::callbacks::process_aux_callbacks;

lazy_static::lazy_static! {
	static ref TURF_CHANNEL: (
		flume::Sender<Box<SSairInfo>>,
		flume::Receiver<Box<SSairInfo>>
	) = flume::bounded(1);
	static ref HEAT_CHANNEL: (flume::Sender<SSheatInfo>, flume::Receiver<SSheatInfo>) =
		flume::bounded(1);
}

static INIT_TURF: Once = Once::new();

static INIT_HEAT: Once = Once::new();

//thread status
static TASKS_RUNNING: AtomicU8 = AtomicU8::new(0);

#[derive(Copy, Clone)]
#[allow(unused)]
struct SSairInfo {
	fdm_max_steps: i32,
	equalize_turf_limit: usize,
	equalize_hard_turf_limit: usize,
	equalize_enabled: bool,
	group_pressure_goal: f32,
	max_x: i32,
	max_y: i32,
	planet_enabled: bool,
}

#[derive(Copy, Clone)]
struct SSheatInfo {
	time_delta: f64,
	max_x: i32,
	max_y: i32,
}

fn with_processing_callback_receiver<T>(f: impl Fn(&flume::Receiver<Box<SSairInfo>>) -> T) -> T {
	f(&TURF_CHANNEL.1)
}

fn processing_callbacks_sender() -> flume::Sender<Box<SSairInfo>> {
	TURF_CHANNEL.0.clone()
}

fn with_heat_processing_callback_receiver<T>(f: impl Fn(&flume::Receiver<SSheatInfo>) -> T) -> T {
	f(&HEAT_CHANNEL.1)
}

fn heat_processing_callbacks_sender() -> flume::Sender<SSheatInfo> {
	HEAT_CHANNEL.0.clone()
}

#[hook("/datum/controller/subsystem/air/proc/thread_running")]
fn _thread_running_hook() {
	Ok(Value::from(TASKS_RUNNING.load(Ordering::Relaxed) != 0))
}

#[hook("/datum/controller/subsystem/air/proc/finish_turf_processing_auxtools")]
fn _finish_process_turfs() {
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
	process_aux_callbacks(crate::callbacks::TURFS);
	if processing_callbacks_unfinished {
		Ok(Value::from(true))
	} else {
		Ok(Value::from(false))
	}
}

#[hook("/datum/controller/subsystem/air/proc/process_turfs_auxtools")]
fn _process_turf_notify() {
	let sender = processing_callbacks_sender();
	let fdm_max_steps = src
		.get_number(byond_string!("share_max_steps"))
		.unwrap_or(1.0) as i32;
	let equalize_turf_limit = src
		.get_number(byond_string!("equalize_turf_limit"))
		.unwrap_or(100.0) as usize;
	let equalize_hard_turf_limit = src
		.get_number(byond_string!("equalize_hard_turf_limit"))
		.unwrap_or(2000.0) as usize;
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
		.unwrap_or(0.5);
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
	let planet_enabled: bool = src
		.get_number(byond_string!("planet_equalize_enabled"))
		.unwrap_or(1.0)
		!= 0.0;
	process_aux_callbacks(crate::callbacks::TURFS);
	let _ = sender.try_send(Box::new(SSairInfo {
		fdm_max_steps,
		equalize_turf_limit,
		equalize_hard_turf_limit,
		equalize_enabled,
		group_pressure_goal,
		max_x,
		max_y,
		planet_enabled,
	}));
	Ok(Value::null())
}

//Fires the task into the thread pool, once
#[init(full)]
fn _process_turf_start() -> Result<(), String> {
	INIT_TURF.call_once(|| {
		#[allow(unused)]
		rayon::spawn(|| loop {
			//this will block until process_turfs is called
			let info = with_processing_callback_receiver(|receiver| receiver.recv().unwrap());
			TASKS_RUNNING.fetch_add(1, Ordering::SeqCst);
			let sender = byond_callback_sender();
			let (low_pressure_turfs, high_pressure_turfs) = {
				let start_time = Instant::now();
				let (low_pressure_turfs, high_pressure_turfs) = fdm(
					info.max_x,
					info.max_y,
					info.fdm_max_steps,
					info.equalize_enabled,
				);
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
				let processed_turfs = excited_group_processing(
					info.max_x,
					info.max_y,
					info.group_pressure_goal,
					&low_pressure_turfs,
				);
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
			if info.equalize_enabled {
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
							planet_enabled,
						)
					}
					#[cfg(feature = "katmos")]
					{
						super::katmos::equalize(
							info.max_x,
							info.max_y,
							info.equalize_hard_turf_limit,
							&high_pressure_turfs,
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
			TASKS_RUNNING.fetch_sub(1, Ordering::SeqCst);
		});
	});
	Ok(())
}

// Compares with neighbors, returning early if any of them are valid.
fn should_process(m: TurfMixture, all_mixtures: &[RwLock<Mixture>]) -> bool {
	m.adjacency > 0
		&& m.enabled()
		&& all_mixtures
			.get(m.mix)
			.and_then(RwLock::try_read)
			.map_or(false, |gas| {
				for entry in m.adjacent_mixes(all_mixtures) {
					if let Some(mix) = entry.try_read() {
						if gas.temperature_compare(&mix)
							|| gas.compare_with(&mix, MINIMUM_MOLES_DELTA_TO_MOVE)
						{
							return true;
						}
					} else {
						return false;
					}
				}
				m.planetary_atmos
					.and_then(|id| planetary_atmos().try_get(&id).try_unwrap())
					.map_or(false, |planet_atmos_entry| {
						let planet_atmos = planet_atmos_entry.value();
						gas.temperature_compare(planet_atmos)
							|| gas.compare_with(planet_atmos, MINIMUM_MOLES_DELTA_TO_MOVE)
					})
			})
}

// Creates the combined gas mixture of all this mix's neighbors, as well as gathering some other pertinent info for future processing.
fn process_cell(
	i: TurfID,
	m: TurfMixture,
	max_x: i32,
	max_y: i32,
	all_mixtures: &[RwLock<Mixture>],
) -> Option<(TurfID, TurfMixture, Mixture, [(TurfID, f32); 6], i32)> {
	let mut adj_amount = 0;
	/*
		Getting write locks is potential danger zone,
		so we make sure we don't do that unless we
		absolutely need to. Saving is fast enough.
	*/
	let mut end_gas = Mixture::from_vol(crate::constants::CELL_VOLUME);
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
	for (j, loc, entry) in m.adjacent_mixes_with_adj_info(all_mixtures, i, max_x, max_y) {
		match entry.try_read() {
			Some(mix) => {
				end_gas.merge(&mix);
				adj_amount += 1;
				pressure_diffs[j as usize] = (loc, -mix.return_pressure() * GAS_DIFFUSION_CONSTANT);
			}
			None => return None, // this would lead to inconsistencies--no bueno
		}
	}
	if let Some(planet_atmos_entry) = m
		.planetary_atmos
		.and_then(|id| planetary_atmos().try_get(&id).try_unwrap())
	{
		end_gas.merge(planet_atmos_entry.value());
		adj_amount += 1;
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
}

// Solving the heat equation using a Finite Difference Method, an iterative stencil loop.
fn fdm(
	max_x: i32,
	max_y: i32,
	fdm_max_steps: i32,
	equalize_enabled: bool,
) -> (BTreeSet<TurfID>, BTreeSet<TurfID>) {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature,
		sleeping turfs, which is why I've renamed it to fdm.
	*/
	let mut low_pressure_turfs: BTreeSet<TurfID> = BTreeSet::new();
	let mut high_pressure_turfs: BTreeSet<TurfID> = BTreeSet::new();
	let mut cur_count = 1;
	loop {
		if cur_count > fdm_max_steps {
			break;
		}
		GasArena::with_all_mixtures(|all_mixtures| {
			let turfs_to_save = turf_gases()
				/*
					This uses DashMap's rayon feature to parallelize the process.
					The speedup gained from this is actually linear
					with the amount of cores the CPU has, which, to be frank,
					is way better than I was expecting, even though this operation
					is technically embarassingly parallel. It'll probably reach
					some maximum due to the global turf mixture lock access,
					but it's already blazingly fast on my i7, so it should be fine.
				*/
				.par_iter()
				.map(|entry| {
					let (&i, &m) = entry.pair();
					(i, m)
				})
				.filter(|&(_, m)| should_process(m, all_mixtures))
				.filter_map(|(i, m)| process_cell(i, m, max_x, max_y, all_mixtures))
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
					all_mixtures.get(m.mix).map(|entry| {
						let mut max_diff = 0.0_f32;
						let moved_pressure = {
							let gas = entry.read();
							gas.return_pressure() * GAS_DIFFUSION_CONSTANT
						};
						for pressure_diff in &mut pressure_diffs {
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
							let gas: &mut Mixture = &mut entry.write();
							gas.multiply(1.0 - (*adj_amount as f32 * GAS_DIFFUSION_CONSTANT));
							gas.merge(end_gas);
						}
						/*
							If there is neither a major pressure difference
							nor are there any visible gases nor does it need
							to react, we're done outright. We don't need
							to do any more and we don't need to send the
							value to byond, so we don't. However, if we do...
						*/
						(*i, pressure_diffs, max_diff)
					})
				})
				.partition(|&(_, _, max_diff)| max_diff <= 5.0);
			//tossing things around is already handled by katmos, so we don't need to do it here.
			if !equalize_enabled {
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
								for &(id, diff) in &pressure_diffs {
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

// Finds small differences in turf pressures and equalizes them.
fn excited_group_processing(
	max_x: i32,
	max_y: i32,
	pressure_goal: f32,
	low_pressure_turfs: &BTreeSet<TurfID>,
) -> usize {
	let mut found_turfs: BTreeSet<TurfID> = BTreeSet::new();
	for &initial_turf in low_pressure_turfs {
		if found_turfs.contains(&initial_turf) {
			continue;
		}
		let initial_mix_ref = {
			let maybe_initial_mix_ref = turf_gases().try_get(&initial_turf).try_unwrap();
			if maybe_initial_mix_ref.is_none() {
				continue;
			}
			*maybe_initial_mix_ref.unwrap()
		};
		let mut border_turfs: VecDeque<(TurfID, TurfMixture)> = VecDeque::with_capacity(40);
		let mut turfs: Vec<TurfMixture> = Vec::with_capacity(200);
		let mut min_pressure = initial_mix_ref.return_pressure();
		let mut max_pressure = min_pressure;
		let mut fully_mixed = Mixture::new();
		border_turfs.push_back((initial_turf, initial_mix_ref));
		found_turfs.insert(initial_turf);
		GasArena::with_all_mixtures(|all_mixtures| {
			loop {
				if turfs.len() >= 2500 {
					break;
				}
				if let Some((i, turf)) = border_turfs.pop_front() {
					let adj_tiles = adjacent_tile_ids(turf.adjacency, i, max_x, max_y);
					if let Some(lock) = all_mixtures.get(turf.mix) {
						let mix = lock.read();
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
							if let Some(border_mix) = turf_gases()
								.try_get(&loc)
								.try_unwrap()
								.filter(|b| b.enabled())
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
				turfs.par_iter().with_min_len(125).for_each(|turf| {
					if let Some(mix_lock) = all_mixtures.get(turf.mix) {
						mix_lock.write().copy_from_mutable(&fully_mixed);
					}
				});
			}
		});
	}
	found_turfs.len()
}

static mut PLANET_RESET_TIMER: Option<Instant> = None;

// If this turf has planetary atmos, and it's sufficiently similar, just sets the turf's atmos to the planetary atmos.
fn remove_trace_planet_gases(
	m: TurfMixture,
	planetary_atmos: &'static DashMap<u32, Mixture, FxBuildHasher>,
	all_mixtures: &[RwLock<Mixture>],
) {
	if let Some(planet_atmos_entry) = m
		.planetary_atmos
		.and_then(|id| planetary_atmos.try_get(&id).try_unwrap())
	{
		let planet_atmos = planet_atmos_entry.value();
		if all_mixtures
			.get(m.mix)
			.and_then(RwLock::try_read)
			.map_or(false, |gas| !gas.compare_with(planet_atmos, 0.1))
		{
			if let Some(mut gas) = all_mixtures.get(m.mix).and_then(|lock| lock.try_write()) {
				gas.copy_from_mutable(planet_atmos);
			}
		}
	}
}

static mut VISUALS_CACHE: Option<std::collections::HashMap<usize, u64, FxBuildHasher>> = None;

// Checks if the gas can react or can update visuals, returns None if not.
fn post_process_cell(
	i: TurfID,
	m: TurfMixture,
	vis: &[Option<f32>],
	all_mixtures: &[RwLock<Mixture>],
	vis_cache: &std::collections::HashMap<usize, u64, FxBuildHasher>,
	updates: &flume::Sender<(usize, u64)>,
) -> Option<(TurfID, bool, bool)> {
	all_mixtures
		.get(m.mix)
		.and_then(RwLock::try_read)
		.and_then(|gas| {
			let should_update_visuals =
				match gas.vis_hash_changed(vis, vis_cache.get(&m.mix).copied().unwrap_or(0)) {
					Ok(hash) => {
						let _ = updates.send((m.mix, hash));
						true
					}
					Err(_) => false,
				};
			let reactable = gas.can_react();
			(should_update_visuals || reactable).then(|| (i, should_update_visuals, reactable))
		})
}

// Goes through every turf, checks if it should reset to planet atmos, if it should
// update visuals, if it should react, sends a callback if it should.
fn post_process() {
	let should_check_planet_turfs = unsafe {
		let timer = PLANET_RESET_TIMER.get_or_insert_with(Instant::now);
		if timer.elapsed() > Duration::from_secs(5) {
			*timer = Instant::now();
			true
		} else {
			false
		}
	};
	let vis = crate::gas::visibility_copies();
	let (sender, receiver) = flume::unbounded();
	let vis_cache = unsafe {
		VISUALS_CACHE
			.get_or_insert_with(|| std::collections::HashMap::with_hasher(FxBuildHasher::default()))
	};
	let processables = {
		turf_gases()
			.par_iter()
			.map(|entry| {
				let (&i, &m) = entry.pair();
				(i, m)
			})
			.filter_map(|(i, m)| {
				m.enabled()
					.then(|| {
						GasArena::with_all_mixtures(|all_mixtures| {
							if should_check_planet_turfs {
								let planetary_atmos = planetary_atmos();
								remove_trace_planet_gases(m, planetary_atmos, all_mixtures);
							}
							post_process_cell(i, m, &vis, all_mixtures, &vis_cache, &sender.clone())
						})
					})
					.flatten()
			})
			.collect::<Vec<_>>()
	};
	processables.into_par_iter().chunks(30).for_each(|chunk| {
		let sender = byond_callback_sender();
		let _ = sender.try_send(Box::new(move || {
			for (i, should_update_vis, should_react) in chunk.clone() {
				let turf = unsafe { Value::turf_by_id_unchecked(i) };
				if should_react {
					if cfg!(target_os = "linux") {
						turf.get(byond_string!("air"))?.call("vv_react", &[&turf])?;
					} else {
						turf.get(byond_string!("air"))?.call("react", &[&turf])?;
					}
				}
				if should_update_vis {
					turf.call("update_visuals", &[])?;
				}
			}
			Ok(Value::null())
		}));
	});
	for (k, v) in receiver.drain() {
		vis_cache.insert(k, v);
	}
}

static HEAT_PROCESS_TIME: AtomicU64 = AtomicU64::new(1_000_000);

#[hook("/datum/controller/subsystem/air/proc/heat_process_time")]
fn _process_heat_time() {
	let tot = HEAT_PROCESS_TIME.load(Ordering::Relaxed);
	Ok(Value::from(
		Duration::new(tot / 1_000_000_000, (tot % 1_000_000_000) as u32).as_millis() as f32,
	))
}

// Expected function call: process_turf_heat()
// Returns: TRUE if thread not done, FALSE otherwise
#[hook("/datum/controller/subsystem/air/proc/process_turf_heat")]
fn _process_heat_notify() {
	/*
		Replacing LINDA's superconductivity system is this much more brute-force
		system--it shares heat between turfs and their neighbors,
		then receives and emits radiation to space, then shares
		between turfs and their gases. Since the latter requires a write lock,
		it's done after the previous step. This one doesn't care about
		consistency like the processing step does--this can run in full parallel.

		Can't get a number from src in the thread, so we get it here.
		Have to get the time delta because the radiation
		is actually physics-based--the stefan boltzmann constant
		and radiation from space both have dimensions of second^-1 that
		need to be multiplied out to have any physical meaning.
		They also have dimensions of meter^-2, but I'm assuming
		turf tiles are 1 meter^2 anyway--the atmos subsystem
		does this in general, thus turf gas mixtures being 2.5 m^3.
	*/
	let sender = heat_processing_callbacks_sender();
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
	process_aux_callbacks(crate::callbacks::TEMPERATURE);
	let _ = sender.try_send(SSheatInfo {
		time_delta,
		max_x,
		max_y,
	});
	Ok(Value::null())
}

//Fires the task into the thread pool, once
#[init(full)]
fn _process_heat_start() -> Result<(), String> {
	INIT_HEAT.call_once(|| {
		rayon::spawn(|| loop {
			//this will block until process_turf_heat is called
			let info = with_heat_processing_callback_receiver(|receiver| receiver.recv().unwrap());
			TASKS_RUNNING.fetch_add(1, Ordering::SeqCst);
			let start_time = Instant::now();
			let sender = byond_callback_sender();
			let emissivity_constant: f64 = STEFAN_BOLTZMANN_CONSTANT * info.time_delta;
			let radiation_from_space_tick: f64 = RADIATION_FROM_SPACE * info.time_delta;
			let temps_to_update = turf_temperatures()
				.par_iter()
				.map(|entry| {
					let (&i, &t) = entry.pair();
					(i, t)
				})
				.filter_map(|(i, t)| {
					let adj = t.adjacency;
					/*
						If it has no thermal conductivity or low thermal capacity,
						then it's not gonna interact, or at least shouldn't.
					*/
					(t.thermal_conductivity > 0.0 && t.heat_capacity > 0.0 && adj > 0)
						.then(|| {
							let mut heat_delta = 0.0;
							let is_temp_delta_with_air = turf_gases()
								.try_get(&i)
								.try_unwrap()
								.filter(|m| m.enabled())
								.and_then(|m| {
									GasArena::with_all_mixtures(|all_mixtures| {
										all_mixtures.get(m.mix).and_then(RwLock::try_read).map(
											|gas| (t.temperature - gas.get_temperature() > 1.0),
										)
									})
								})
								.unwrap_or(false);
							for (_, loc) in adjacent_tile_ids(adj, i, info.max_x, info.max_y) {
								heat_delta += turf_temperatures().try_get(&loc).try_unwrap().map_or(
									0.0,
									|other| {
										/*
											The horrible line below is essentially
											sharing between solids--making it the minimum of both
											conductivities makes this consistent, funnily enough.
										*/
										t.thermal_conductivity.min(other.thermal_conductivity)
											* (other.temperature - t.temperature) * (t.heat_capacity
											* other.heat_capacity
											/ (t.heat_capacity + other.heat_capacity))
									},
								)
							}
							if t.adjacent_to_space {
								/*
									Straight up the standard blackbody radiation
									equation. All these are f64s because
									f32::MAX^4 < f64::MAX, and t.temperature
									is ordinarily an f32, meaning that
									this will never go into infinities.
								*/
								let blackbody_radiation: f64 = (emissivity_constant
									* ((t.temperature as f64).powi(4)))
									- radiation_from_space_tick;
								heat_delta -= blackbody_radiation as f32;
							}
							let temp_delta = heat_delta / t.heat_capacity;
							(is_temp_delta_with_air || temp_delta.abs() > 0.01)
								.then(|| (i, t.temperature + temp_delta))
						})
						.flatten()
				})
				.collect::<Vec<_>>();
			temps_to_update
				.par_iter()
				.with_min_len(100)
				.for_each(|&(i, new_temp)| {
					let maybe_t = turf_temperatures().try_get_mut(&i).try_unwrap();
					if maybe_t.is_none() {
						return;
					}
					let t: &mut ThermalInfo = &mut maybe_t.unwrap();
					t.temperature = turf_gases()
						.try_get(&i)
						.try_unwrap()
						.filter(|m| m.enabled())
						.and_then(|m| {
							GasArena::with_all_mixtures(|all_mixtures| {
								all_mixtures.get(m.mix).map(|entry| {
									let gas: &mut Mixture = &mut entry.write();
									gas.temperature_share_non_gas(
										/*
											This value should be lower than the
											turf-to-turf conductivity for balance reasons
											as well as realism, otherwise fires will
											just sort of solve theirselves over time.
										*/
										t.thermal_conductivity * OPEN_HEAT_TRANSFER_COEFFICIENT,
										new_temp,
										t.heat_capacity,
									)
								})
							})
						})
						.unwrap_or(new_temp);
					if !t.temperature.is_normal() {
						t.temperature = 2.7;
					}
					if t.temperature > MINIMUM_TEMPERATURE_START_SUPERCONDUCTION
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
			//Alright, now how much time did that take?
			let bench = start_time.elapsed().as_nanos();
			let old_bench = HEAT_PROCESS_TIME.load(Ordering::SeqCst);
			// We display this as part of the MC atmospherics stuff.
			HEAT_PROCESS_TIME.store((old_bench * 3 + (bench * 7) as u64) / 10, Ordering::SeqCst);
			TASKS_RUNNING.fetch_sub(1, Ordering::SeqCst);
		});
	});
	Ok(())
}

#[shutdown]
fn reset_auxmos_processing() {
	unsafe { VISUALS_CACHE = None };
	HEAT_PROCESS_TIME.store(1_000_000, Ordering::Relaxed);
}
