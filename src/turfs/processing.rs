use byondapi::prelude::*;

use super::*;

use crate::GasArena;

use auxcallback::{byond_callback_sender, process_callbacks_for_millis};

use parking_lot::RwLock;

use tinyvec::TinyVec;

use std::collections::{BTreeMap, BTreeSet};

use coarsetime::{Duration, Instant};

#[byondapi_binds::bind("/datum/controller/subsystem/air/proc/thread_running")]
fn thread_running_hook() {
	Ok(TASKS.try_write().is_none().into())
}

#[byondapi_binds::bind("/datum/controller/subsystem/air/proc/finish_turf_processing_auxtools")]
fn finish_process_turfs(time_remaining: ByondValue) {
	Ok(process_callbacks_for_millis(time_remaining.get_number()? as u64).into())
}

#[byondapi_binds::bind("/datum/controller/subsystem/air/proc/process_turfs_auxtools")]
fn process_turf_hook(src: ByondValue, remaining: ByondValue) {
	let remaining_time = Duration::from_millis(remaining.get_number().unwrap_or(50.0) as u64);
	let fdm_max_steps = src.read_number("share_max_steps").unwrap_or(1.0) as i32;
	let equalize_enabled = cfg!(feature = "fastmos") && src.read_number("equalize_enabled")? != 0.0;

	process_turf(remaining_time, fdm_max_steps, equalize_enabled, src)?;
	Ok(ByondValue::null())
}

fn process_turf(
	remaining: Duration,
	fdm_max_steps: i32,
	equalize_enabled: bool,
	ssair: ByondValue,
) -> Result<()> {
	//this will block until process_turfs is called
	let (low_pressure_turfs, _high_pressure_turfs) = {
		let start_time = Instant::now();
		let (low_pressure_turfs, high_pressure_turfs) =
			fdm((&start_time, remaining), fdm_max_steps, equalize_enabled);
		let bench = start_time.elapsed().as_millis();
		let (lpt, hpt) = (low_pressure_turfs.len(), high_pressure_turfs.len());
		let prev_cost = ssair.read_number("cost_turfs")?;
		ssair
			.read_var("cost_turfs")?
			.set_number(0.8 * prev_cost + 0.2 * (bench as f32));
		ssair.read_var("low_pressure_turfs")?.set_number(lpt as f32);
		ssair
			.read_var("high_pressure_turfs")?
			.set_number(hpt as f32);
		(low_pressure_turfs, high_pressure_turfs)
	};
	{
		let start_time = Instant::now();
		post_process();
		let bench = start_time.elapsed().as_millis();
		let prev_cost = ssair.read_number("cost_post_process")?;
		ssair
			.read_var("cost_post_process")?
			.set_number(0.8 * prev_cost + 0.2 * (bench as f32));
	}
	{
		planet_process();
	}
	{
		super::groups::send_to_groups(low_pressure_turfs);
	}
	if equalize_enabled {
		#[cfg(feature = "fastmos")]
		{
			super::katmos::send_to_equalize(_high_pressure_turfs);
		}
	}
	Ok(())
}

fn planet_process() {
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
							let has_temp_difference = gas_read.temperature_compare(planet_atmos);
							if let Some(mut gas) = (has_temp_difference
								|| (comparison > GAS_MIN_MOLES))
								.then(|| {
									parking_lot::lock_api::RwLockUpgradableReadGuard::try_upgrade(
										gas_read,
									)
									.ok()
								})
								.flatten()
							{
								if comparison > 0.1 || has_temp_difference {
									gas.share_ratio(planet_atmos, GAS_DIFFUSION_CONSTANT);
								} else {
									gas.copy_from_mutable(planet_atmos);
								}
							}
						}
					})
			})
		})
	});
}

// Compares with neighbors, returning early if any of them are valid.
fn should_process(
	index: NodeIndex,
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
	index: NodeIndex,
	all_mixtures: &[RwLock<Mixture>],
	arena: &TurfGases,
) -> Option<(NodeIndex, Mixture, TinyVec<[(TurfID, f32); 6]>, i32)> {
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
	(start_time, remaining_time): (&Instant, Duration),
	fdm_max_steps: i32,
	equalize_enabled: bool,
) -> (BTreeSet<TurfID>, BTreeSet<TurfID>) {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature,
		sleeping turfs, which is why I've renamed it to fdm.
	*/
	let mut low_pressure_turfs: BTreeSet<TurfID> = Default::default();
	let mut high_pressure_turfs: BTreeSet<TurfID> = Default::default();
	let mut cur_count = 1;
	with_turf_gases_read(|arena| {
		loop {
			if cur_count > fdm_max_steps || start_time.elapsed() >= remaining_time {
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
							(m.id, pressure_diffs, max_diff, i)
						})
					})
					.partition(|&(_, _, max_diff, _)| max_diff <= 5.0);

				high_pressure_turfs.par_extend(high_pressure.par_iter().map(|(i, _, _, _)| i));
				low_pressure_turfs.par_extend(low_pressure.par_iter().map(|(i, _, _, _)| i));
				//tossing things around is already handled by katmos, so we don't need to do it here.
				if !equalize_enabled {
					high_pressure
						.into_par_iter()
						.filter_map(|(_, pressures, _, node_id)| {
							Some((arena.get(node_id)?.id, pressures))
						})
						.for_each(|(id, diffs)| {
							let sender = byond_callback_sender();
							drop(sender.try_send(Box::new(move || {
								let turf = ByondValue::new_ref(0x01, id);
								for (id, diff) in diffs.iter().copied() {
									if id != 0 {
										let enemy_tile = ByondValue::new_ref(0x01, id);
										if diff > 5.0 {
											turf.call(
												"consider_pressure_difference",
												&[enemy_tile, ByondValue::from(diff)],
											)?;
										} else if diff < -5.0 {
											enemy_tile.call(
												"consider_pressure_difference",
												&[turf.clone(), ByondValue::from(-diff)],
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

// Checks if the gas can react or can update visuals, returns None if not.
fn post_process_cell<'a>(
	mixture: &'a TurfMixture,
	vis: &[Option<f32>],
	all_mixtures: &[RwLock<Mixture>],
	reactions: &BTreeMap<crate::reaction::ReactionPriority, crate::reaction::Reaction>,
) -> Option<(&'a TurfMixture, bool, bool)> {
	all_mixtures
		.get(mixture.mix)
		.and_then(RwLock::try_read)
		.and_then(|gas| {
			let should_update_visuals = gas.vis_hash_changed(vis, &mixture.vis_hash);
			let reactable = gas.can_react_with_reactions(reactions);
			(should_update_visuals || reactable).then_some((
				mixture,
				should_update_visuals,
				reactable,
			))
		})
}

// Goes through every turf, checks if it should reset to planet atmos, if it should
// update visuals, if it should react, sends a callback if it should.
fn post_process() {
	let vis = crate::gas::visibility_copies();
	with_turf_gases_read(|arena| {
		let processables = crate::gas::types::with_reactions(|reactions| {
			GasArena::with_all_mixtures(|all_mixtures| {
				Some(
					arena
						.map
						.par_values()
						.filter_map(|&node_index| {
							let mix = arena.get(node_index).unwrap();
							mix.enabled().then_some(mix)
						})
						.filter_map(|mixture| {
							post_process_cell(mixture, &vis, all_mixtures, reactions)
						})
						.collect::<Vec<_>>(),
				)
			})
		});
		if processables.is_none() {
			return;
		}
		processables.unwrap().into_par_iter().for_each(
			|(tmix, should_update_vis, should_react)| {
				let sender = byond_callback_sender();
				let id = tmix.id;

				if should_react {
					drop(sender.try_send(Box::new(move || {
						let turf = ByondValue::new_ref(0x01, id);
						if cfg!(target_os = "linux") {
							turf.read_var("air")?.call("vv_react", &[turf])?;
						} else {
							turf.read_var("air")?.call("react", &[turf])?;
						}
						Ok(())
					})));
				}

				if should_update_vis
					&& sender
						.try_send(Box::new(move || {
							let turf = ByondValue::new_ref(0x01, id);
							update_visuals(turf)?;
							Ok(())
						}))
						.is_err()
				{
					//this update failed, consider vis_cache to be bogus so it can send the
					//update again later
					tmix.invalidate_vis_cache();
				}
			},
		);
	});
}
