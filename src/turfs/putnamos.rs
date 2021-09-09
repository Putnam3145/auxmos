use super::*;

use std::collections::VecDeque;

use std::collections::{BTreeMap, BTreeSet};

use std::cell::Cell;

use auxcallback::byond_callback_sender;

const OPP_DIR_INDEX: [u8; 7] = [1, 0, 3, 2, 5, 4, 6];

// If you can't tell, this is mostly a massively simplified copy of monstermos.

fn explosively_depressurize(
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
			actual_turf.set(byond_string!("pressure_specific_target"), &actual_turf)?;
		} else {
			if cur_queue_idx > equalize_hard_turf_limit {
				continue;
			}
			for (j, loc) in adjacent_tile_ids(m.adjacency, i, max_x, max_y) {
				actual_turf.call(
					"consider_firelocks",
					&[&unsafe { Value::turf_by_id_unchecked(loc) }],
				)?;
				if let Some(new_m) = turf_gases().get(&i) {
					let bit = 1 << j;
					if new_m.adjacency & bit == bit {
						if let Some(adj) = turf_gases().get(&loc) {
							let (&adj_i, &adj_m) = (adj.key(), adj.value());
							turfs.push((adj_i, adj_m));
						}
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
			if let Some(adj) = turf_gases().get(&loc) {
				let (adj_i, adj_m) = (*adj.key(), adj.value());
				if !adjacency_info.contains_key(&adj_i) && !adj_m.is_immutable() {
					adjacency_info.insert(i, Cell::new((OPP_DIR_INDEX[j as usize], 0.0)));
					unsafe { Value::turf_by_id_unchecked(adj_i) }
						.set(byond_string!("pressure_specific_target"), &actual_turf)?;
					progression_order.push((adj_i, *adj_m));
				}
			}
		}
		cur_queue_idx += 1;
	}
	let hpd = auxtools::Value::globals()
		.get(byond_string!("SSAir"))?
		.get_list(byond_string!("high_pressure_delta"))
		.map_err(|_| {
			runtime!(
				"Attempt to interpret non-list value as list {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})?;
	for (i, m) in progression_order.iter().rev() {
		let cur_orig = adjacency_info.get(i).unwrap_or_default();
		let mut cur_info = cur_orig.get();
		if cur_info.0 == 6 {
			continue;
		}
		let actual_turf = unsafe { Value::turf_by_id_unchecked(*i) };
		hpd.set(&actual_turf, 1.0)?;
		let loc = adjacent_tile_id(cur_info.0, *i, max_x, max_y);
		if let Some(adj) = turf_gases().get(&loc) {
			let (adj_i, adj_m) = (*adj.key(), adj.value());
			let adj_orig = adjacency_info.get(&adj_i).unwrap_or_default();
			let mut adj_info = adj_orig.get();
			let sum = adj_m.total_moles();
			cur_info.1 += sum;
			adj_info.1 += cur_info.1;
			if adj_info.0 != 6 {
				let adj_turf = unsafe { Value::turf_by_id_unchecked(adj_i) };
				adj_turf.set(byond_string!("pressure_difference"), cur_info.1)?;
				adj_turf.set(
					byond_string!("pressure_direction"),
					(1 << cur_info.0) as f32,
				)?;
			}
			m.clear_air();
			actual_turf.set(byond_string!("pressure_difference"), cur_info.1)?;
			actual_turf.set(
				byond_string!("pressure_direction"),
				(1 << cur_info.0) as f32,
			)?;
			actual_turf.call("handle decompression floor rip", &[&Value::from(sum)])?;
			adj_orig.set(adj_info);
			cur_orig.set(cur_info);
		}
	}
	Ok(Value::null())
}

// Just floodfills to lower-pressure turfs until it can't find any more.

#[deprecated(note = "Prefer monstermos.")]
pub fn equalize(
	equalize_turf_limit: usize,
	equalize_hard_turf_limit: usize,
	max_x: i32,
	max_y: i32,
	high_pressure_turfs: BTreeSet<TurfID>,
) -> usize {
	let sender = byond_callback_sender();
	let mut turfs_processed = 0;
	let mut merger = Mixture::new();
	let mut found_turfs: BTreeSet<TurfID> = BTreeSet::new();
	'turf_loop: for &initial_idx in high_pressure_turfs.iter() {
		if let Some(initial_turf) = turf_gases().get(&initial_idx) {
			let mut turfs: Vec<(TurfID, TurfMixture, TurfID, f32)> =
				Vec::with_capacity(equalize_turf_limit);
			let mut border_turfs: VecDeque<(TurfID, TurfMixture, TurfID, f32)> =
				VecDeque::with_capacity(equalize_turf_limit);
			merger.clear_with_vol(0.0);
			border_turfs.push_back((initial_idx, *initial_turf, initial_idx, 0.0));
			found_turfs.insert(initial_idx);
			if GasMixtures::with_all_mixtures(|all_mixtures| {
				// floodfill
				while !border_turfs.is_empty() && turfs.len() < equalize_turf_limit {
					let (cur_idx, cur_turf, parent_turf, pressure_delta) =
						border_turfs.pop_front().unwrap();
					if let Some(our_gas_entry) = all_mixtures.get(cur_turf.mix) {
						let gas = our_gas_entry.read();
						merger.merge(&gas);
						merger.volume += gas.volume;
						turfs.push((cur_idx, cur_turf, parent_turf, pressure_delta));
						if !gas.is_immutable() {
							for (_, loc) in
								adjacent_tile_ids(cur_turf.adjacency, cur_idx, max_x, max_y)
							{
								if found_turfs.contains(&loc) {
									continue;
								}
								found_turfs.insert(loc);
								if let Some(adj_turf) = turf_gases().get(&loc) {
									if cfg!(feature = "putnamos_decompression")
										&& adj_turf.is_immutable()
									{
										let _ = sender.try_send(Box::new(move || {
											explosively_depressurize(
												cur_idx,
												cur_turf,
												equalize_hard_turf_limit,
												max_x,
												max_y,
											)
										}));
										return true;
									} else {
										let delta =
											adj_turf.return_pressure() - merger.return_pressure();
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
					}
				}
				false
			}) || turfs.len() == 1
			{
				continue 'turf_loop;
			}
			merger.multiply(1.0 / turfs.len() as f32);
			turfs_processed += turfs.len();
			let to_send = GasMixtures::with_all_mixtures(|all_mixtures| {
				turfs
					.par_iter()
					.with_min_len(50)
					.map(|(cur_idx, cur_turf, parent_turf, pressure_delta)| {
						if let Some(entry) = all_mixtures.get(cur_turf.mix) {
							let gas: &mut Mixture = &mut entry.write();
							gas.copy_from_mutable(&merger);
						}
						(*cur_idx, *parent_turf, *pressure_delta)
					})
					.collect::<Vec<_>>()
			});
			for chunk_prelude in to_send.chunks(20) {
				let chunk: Vec<_> = chunk_prelude.iter().copied().collect();
				let _ = sender.try_send(Box::new(move || {
					for &(idx, parent, delta) in chunk.iter() {
						if parent != 0 {
							let turf = unsafe { Value::turf_by_id_unchecked(idx) };
							let enemy_turf = unsafe { Value::turf_by_id_unchecked(parent) };
							enemy_turf.call(
								"consider_pressure_difference",
								&[&turf, &Value::from(delta)],
							)?;
						}
					}
					Ok(Value::null())
				}));
			}
		}
	}
	turfs_processed
}
