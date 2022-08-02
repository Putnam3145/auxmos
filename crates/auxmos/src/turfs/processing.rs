use auxtools::*;

use super::*;

use crate::GasArena;

use auxcallback::{byond_callback_sender, process_callbacks_for_millis};

use parking_lot::{Once, RwLock};

use tinyvec::TinyVec;

use std::{
	collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
	time::Instant,
};

static INIT_TURF: Once = Once::new();

lazy_static::lazy_static! {
	static ref TURF_CHANNEL: (
		flume::Sender<Box<SSairInfo>>,
		flume::Receiver<Box<SSairInfo>>
	) = flume::bounded(1);
}

#[derive(Copy, Clone)]
#[allow(unused)]
struct SSairInfo {
	fdm_max_steps: i32,
	equalize_turf_limit: usize,
	equalize_hard_turf_limit: usize,
	equalize_enabled: bool,
	group_pressure_goal: f32,
	planet_enabled: bool,
}

fn with_processing_callback_receiver<T>(f: impl Fn(&flume::Receiver<Box<SSairInfo>>) -> T) -> T {
	f(&TURF_CHANNEL.1)
}

fn processing_callbacks_sender() -> flume::Sender<Box<SSairInfo>> {
	TURF_CHANNEL.0.clone()
}

#[hook("/datum/controller/subsystem/air/proc/thread_running")]
fn _thread_running_hook() {
	Ok(Value::from(TASKS.try_write().is_none()))
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
	if TASKS.try_write().is_some() {
		rebuild_turf_graph()?;
	}
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
	let equalize_enabled = cfg!(feature = "fastmos")
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
	let planet_enabled: bool = src
		.get_number(byond_string!("planet_equalize_enabled"))
		.unwrap_or(1.0)
		!= 0.0;
	drop(sender.try_send(Box::new(SSairInfo {
		fdm_max_steps,
		equalize_turf_limit,
		equalize_hard_turf_limit,
		equalize_enabled,
		group_pressure_goal,
		planet_enabled,
	})));
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
			let task_lock = TASKS.read();
			let sender = byond_callback_sender();
			let mut stats: Vec<Box<dyn Fn() -> Result<(), Runtime> + Send + Sync>> =
				Default::default();
			let (low_pressure_turfs, high_pressure_turfs) = {
				let start_time = Instant::now();
				let (low_pressure_turfs, high_pressure_turfs) =
					fdm(info.fdm_max_steps, info.equalize_enabled);
				let bench = start_time.elapsed().as_millis();
				let (lpt, hpt) = (low_pressure_turfs.len(), high_pressure_turfs.len());
				stats.push(Box::new(move || {
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
					Ok(())
				}));
				(low_pressure_turfs, high_pressure_turfs)
			};
			{
				let start_time = Instant::now();
				let processed_turfs =
					excited_group_processing(info.group_pressure_goal, &low_pressure_turfs);
				let bench = start_time.elapsed().as_millis();
				stats.push(Box::new(move || {
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
					Ok(())
				}));
			}
			if info.equalize_enabled {
				let start_time = Instant::now();
				let processed_turfs = {
					#[cfg(feature = "fastmos")]
					{
						super::katmos::equalize(
							info.equalize_hard_turf_limit,
							&high_pressure_turfs,
							info.planet_enabled,
						)
					}
					#[cfg(not(feature = "fastmos"))]
					{
						0
					}
				};
				let bench = start_time.elapsed().as_millis();
				stats.push(Box::new(move || {
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
					Ok(())
				}));
			}
			{
				let start_time = Instant::now();
				post_process();
				let bench = start_time.elapsed().as_millis();
				stats.push(Box::new(move || {
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
					Ok(())
				}));
			}
			{
				drop(sender.try_send(Box::new(move || {
					for callback in stats.iter() {
						callback()?;
					}
					Ok(())
				})));
			}
			{
				//let it gooooo
				rayon::spawn(planet_process);
			}
			drop(task_lock);
		});
	});
	Ok(())
}

fn planet_process() {
	let task_lock = TASKS.read();
	with_turf_gases_read(|arena| {
		GasArena::with_all_mixtures(|all_mixtures| {
			with_planetary_atmos(|map| {
				arena
					.map
					.par_values()
					.filter_map(|&node_idx| {
						let mix = arena.get(node_idx)?;
						Some((mix, mix.planetary_atmos.and_then(|id| map.get(&id))?))
					})
					.for_each(|(turf_mix, planet_atmos)| {
						if let Some(gas_read) = all_mixtures
							.get(turf_mix.mix)
							.and_then(|lock| lock.try_upgradable_read())
						{
							let comparison = gas_read.compare(planet_atmos);
							if let Some(mut gas) = (comparison > GAS_MIN_MOLES)
								.then(|| {
									parking_lot::lock_api::RwLockUpgradableReadGuard::try_upgrade(
										gas_read,
									)
									.ok()
								})
								.flatten()
							{
								if comparison > 0.1 {
									gas.multiply(1.0 - GAS_DIFFUSION_CONSTANT);
									gas.merge(&(planet_atmos * GAS_DIFFUSION_CONSTANT));
								} else {
									gas.copy_from_mutable(planet_atmos);
								}
							}
						}
					})
			})
		})
	});
	drop(task_lock)
}

// Compares with neighbors, returning early if any of them are valid.
fn should_process(
	index: NodeIndex<usize>,
	mixture: &TurfMixture,
	all_mixtures: &[RwLock<Mixture>],
	arena: &TurfGases,
) -> bool {
	mixture.enabled()
		&& arena.adjacent_node_ids(index).next().is_some()
		&& all_mixtures
			.get(mixture.mix)
			.and_then(RwLock::try_read)
			.map_or(false, |gas| {
				for entry in arena.adjacent_mixes(index, all_mixtures) {
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
				false
			})
}

// Creates the combined gas mixture of all this mix's neighbors, as well as gathering some other pertinent info for future processing.
// Clippy go away, this type is only used once
#[allow(clippy::type_complexity)]
fn process_cell(
	index: NodeIndex<usize>,
	all_mixtures: &[RwLock<Mixture>],
	arena: &TurfGases,
) -> Option<(NodeIndex<usize>, Mixture, TinyVec<[(TurfID, f32); 6]>, i32)> {
	let mut adj_amount = 0;
	/*
		Getting write locks is potential danger zone,
		so we make sure we don't do that unless we
		absolutely need to. Saving is fast enough.
	*/
	let mut end_gas = Mixture::from_vol(crate::constants::CELL_VOLUME);
	let mut pressure_diffs: TinyVec<[(TurfID, f32); 6]> = Default::default();
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
	for (&loc, entry) in
		arena.adjacent_mixes_with_adj_ids(index, all_mixtures, petgraph::Direction::Incoming)
	{
		match entry.try_read() {
			Some(mix) => {
				end_gas.merge(&mix);
				adj_amount += 1;
				pressure_diffs.push((loc, -mix.return_pressure() * GAS_DIFFUSION_CONSTANT));
			}
			None => return None, // this would lead to inconsistencies--no bueno
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
	Some((index, end_gas, pressure_diffs, adj_amount))
}

// Solving the heat equation using a Finite Difference Method, an iterative stencil loop.
fn fdm(
	fdm_max_steps: i32,
	equalize_enabled: bool,
) -> (BTreeSet<NodeIndex<usize>>, BTreeSet<NodeIndex<usize>>) {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature,
		sleeping turfs, which is why I've renamed it to fdm.
	*/
	let mut low_pressure_turfs: BTreeSet<NodeIndex<usize>> = Default::default();
	let mut high_pressure_turfs: BTreeSet<NodeIndex<usize>> = Default::default();
	let mut cur_count = 1;
	with_turf_gases_read(|arena| {
		loop {
			if cur_count > fdm_max_steps || check_turfs_dirty() {
				break;
			}
			GasArena::with_all_mixtures(|all_mixtures| {
				let turfs_to_save = arena
					.map
					/*
						This directly yanks the internal node vec
						of the graph as a slice to parallelize the process.
						The speedup gained from this is actually linear
						with the amount of cores the CPU has, which, to be frank,
						is way better than I was expecting, even though this operation
						is technically embarassingly parallel. It'll probably reach
						some maximum due to the global turf mixture lock access,
						but it's already blazingly fast on my i7, so it should be fine.
					*/
					.par_values()
					.map(|&idx| (idx, arena.get(idx).unwrap()))
					.filter(|(index, mixture)| should_process(*index, mixture, all_mixtures, arena))
					.filter_map(|(index, _)| process_cell(index, all_mixtures, arena))
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
					.into_par_iter()
					.filter_map(|(i, end_gas, mut pressure_diffs, adj_amount)| {
						let m = arena.get(i).unwrap();
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
								gas.multiply(1.0 - (adj_amount as f32 * GAS_DIFFUSION_CONSTANT));
								gas.merge(&end_gas);
							}
							/*
								If there is neither a major pressure difference
								nor are there any visible gases nor does it need
								to react, we're done outright. We don't need
								to do any more and we don't need to send the
								value to byond, so we don't. However, if we do...
							*/
							(i, pressure_diffs, max_diff)
						})
					})
					.partition(|&(_, _, max_diff)| max_diff <= 5.0);

				high_pressure_turfs.par_extend(high_pressure.par_iter().map(|(i, _, _)| i));
				low_pressure_turfs.par_extend(low_pressure.par_iter().map(|(i, _, _)| i));
				//tossing things around is already handled by katmos, so we don't need to do it here.
				if !equalize_enabled {
					high_pressure
						.into_par_iter()
						.filter_map(|(node_id, pressures, _)| {
							Some((arena.get(node_id)?.id, pressures))
						})
						.for_each(|(id, diffs)| {
							let sender = byond_callback_sender();
							drop(sender.try_send(Box::new(move || {
								let turf = unsafe { Value::turf_by_id_unchecked(id) };
								for (id, diff) in diffs.to_vec() {
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
								Ok(())
							})));
						});
				}
			});
			cur_count += 1;
		}
	});
	(low_pressure_turfs, high_pressure_turfs)
}

// Finds small differences in turf pressures and equalizes them.
fn excited_group_processing(
	pressure_goal: f32,
	low_pressure_turfs: &BTreeSet<NodeIndex<usize>>,
) -> usize {
	let mut found_turfs: HashSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
	with_turf_gases_read(|arena| {
		for &initial_turf in low_pressure_turfs.iter() {
			if found_turfs.contains(&initial_turf) {
				continue;
			}
			let initial_mix_ref = arena.get(initial_turf).unwrap();
			let mut border_turfs: VecDeque<NodeIndex<usize>> = VecDeque::with_capacity(40);
			let mut turfs: Vec<&TurfMixture> = Vec::with_capacity(200);
			let mut min_pressure = initial_mix_ref.return_pressure();
			let mut max_pressure = min_pressure;
			let mut fully_mixed = Mixture::new();
			border_turfs.push_back(initial_turf);
			found_turfs.insert(initial_turf);
			GasArena::with_all_mixtures(|all_mixtures| {
				loop {
					if turfs.len() >= 2500 || check_turfs_dirty() {
						break;
					}
					if let Some(idx) = border_turfs.pop_front() {
						let tmix = arena.get(idx).unwrap();
						if let Some(lock) = all_mixtures.get(tmix.mix) {
							let mix = lock.read();
							let pressure = mix.return_pressure();
							let this_max = max_pressure.max(pressure);
							let this_min = min_pressure.min(pressure);
							if (this_max - this_min).abs() >= pressure_goal {
								continue;
							}
							min_pressure = this_min;
							max_pressure = this_max;
							turfs.push(tmix);
							fully_mixed.merge(&mix);
							fully_mixed.volume += mix.volume;
							for loc in arena.adjacent_node_ids(idx) {
								if found_turfs.contains(&loc) {
									continue;
								}
								found_turfs.insert(loc);
								if arena.get(loc).filter(|b| b.enabled()).is_some() {
									border_turfs.push_back(loc);
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
	});
	found_turfs.len()
}

// Checks if the gas can react or can update visuals, returns None if not.
fn post_process_cell<'a>(
	mixture: &'a TurfMixture,
	vis: &[Option<f32>],
	all_mixtures: &[RwLock<Mixture>],
	reactions: &BTreeMap<i32, crate::reaction::Reaction>,
) -> Option<(&'a TurfMixture, bool, bool)> {
	all_mixtures
		.get(mixture.mix)
		.and_then(RwLock::try_read)
		.and_then(|gas| {
			let should_update_visuals = gas.vis_hash_changed(vis, &mixture.vis_hash);
			let reactable = gas.can_react_with_reactions(reactions);
			(should_update_visuals || reactable)
				.then(|| (mixture, should_update_visuals, reactable))
		})
}

// Goes through every turf, checks if it should reset to planet atmos, if it should
// update visuals, if it should react, sends a callback if it should.
fn post_process() {
	let vis = crate::gas::visibility_copies();
	with_turf_gases_read(|arena| {
		let processables = crate::gas::types::with_reactions(|reactions| {
			GasArena::with_all_mixtures(|all_mixtures| {
				arena
					.map
					.par_values()
					.filter_map(|&node_index| {
						let mix = arena.get(node_index).unwrap();
						mix.enabled().then(|| mix)
					})
					.filter_map(|mixture| post_process_cell(mixture, &vis, all_mixtures, reactions))
					.collect::<Vec<_>>()
			})
		});
		processables
			.into_par_iter()
			.for_each(|(tmix, should_update_vis, should_react)| {
				let sender = byond_callback_sender();
				let id = tmix.id;

				if should_react {
					drop(sender.try_send(Box::new(move || {
						let turf = unsafe { Value::turf_by_id_unchecked(id) };
						if cfg!(target_os = "linux") {
							turf.get(byond_string!("air"))?.call("vv_react", &[&turf])?;
						} else {
							turf.get(byond_string!("air"))?.call("react", &[&turf])?;
						}
						Ok(())
					})));
				}

				if should_update_vis {
					if sender
						.try_send(Box::new(move || {
							let turf = unsafe { Value::turf_by_id_unchecked(id) };
							update_visuals(turf)?;
							Ok(())
						}))
						.is_err()
					{
						//this update failed, consider vis_cache to be bogus so it can send the
						//update again later
						tmix.invalidate_vis_cache();
					}
				}
			});
	});
}
