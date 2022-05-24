//Monstermos, but zoned, and multithreaded!

use super::*;

use std::{
	cell::Cell,
	{
		collections::{HashMap, HashSet},
		sync::atomic::AtomicUsize,
	},
};

use indexmap::{IndexMap, IndexSet};

use ahash::RandomState;
use fxhash::FxBuildHasher;

use crate::callbacks::process_aux_callbacks;

use auxcallback::byond_callback_sender;

use parking_lot::RwLockReadGuard;

type MixWithID = (NodeIndex, TurfMixture);

type RefMixWithID<'a> = (&'a NodeIndex, &'a TurfMixture);

use petgraph::{graph::NodeIndex, graphmap::DiGraphMap};

#[derive(Copy, Clone)]
struct MonstermosInfo {
	mole_delta: f32,
	curr_transfer_amount: f32,
	curr_transfer_dir: Option<NodeIndex>,
	fast_done: bool,
}

impl Default for MonstermosInfo {
	fn default() -> MonstermosInfo {
		MonstermosInfo {
			mole_delta: 0_f32,
			curr_transfer_amount: 0_f32,
			curr_transfer_dir: None,
			fast_done: false,
		}
	}
}

#[derive(Copy, Clone)]
struct ReducedInfo {
	curr_transfer_amount: f32,
	curr_transfer_dir: Option<NodeIndex>,
}

impl Default for ReducedInfo {
	fn default() -> ReducedInfo {
		ReducedInfo {
			curr_transfer_amount: 0_f32,
			curr_transfer_dir: None,
		}
	}
}

fn adjust_eq_movement(
	this_turf: Option<NodeIndex>,
	that_turf: Option<NodeIndex>,
	amount: f32,
	graph: &mut DiGraphMap<Option<NodeIndex>, f32>,
) {
	if graph.contains_edge(this_turf, that_turf) {
		*graph.edge_weight_mut(this_turf, that_turf).unwrap() += amount;
	} else {
		graph.add_edge(this_turf, that_turf, amount);
	}
	if that_turf.is_some() {
		if graph.contains_edge(that_turf, this_turf) {
			*graph.edge_weight_mut(that_turf, this_turf).unwrap() -= amount;
		} else {
			graph.add_edge(that_turf, this_turf, -amount);
		}
	}
}

fn finalize_eq(
	index: NodeIndex,
	turf: &TurfMixture,
	arena: &RwLockReadGuard<TurfGases>,
	info: &HashMap<NodeIndex, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex>, f32>,
) {
	let sender = byond_callback_sender();
	let transfer_dirs = {
		let pairs = eq_movement_graph
			.edges(Some(index))
			.map(|(_, opt2, amt)| (opt2, *amt))
			.collect::<HashMap<_, _, FxBuildHasher>>();

		if pairs.is_empty() {
			return;
		}
		// null it out to prevent infinite recursion.
		pairs.iter().for_each(|(that_node, _)| {
			eq_movement_graph.remove_edge(Some(index), *that_node);
		});
		pairs
	};
	if let Some(&planet_transfer_amount) = transfer_dirs.get(&None) {
		if planet_transfer_amount > 0.0 {
			if turf.total_moles() < planet_transfer_amount {
				finalize_eq_neighbors(index, arena, &transfer_dirs, info, eq_movement_graph);
			}
			drop(GasArena::with_gas_mixture_mut(turf.mix, |gas| {
				gas.add(-planet_transfer_amount);
				Ok(())
			}));
		} else if planet_transfer_amount < 0.0 {
			if let Some(air_entry) = turf
				.planetary_atmos
				.and_then(|i| planetary_atmos().try_get(&i).try_unwrap())
			{
				let planet_air = air_entry.value();
				let planet_sum = planet_air.total_moles();
				if planet_sum > 0.0 {
					drop(GasArena::with_gas_mixture_mut(turf.mix, |gas| {
						gas.merge(&(planet_air * (-planet_transfer_amount / planet_sum)));
						Ok(())
					}));
				}
			}
		}
	}
	let cur_turf_id = arena.get(index).unwrap().id;
	for adj_index in arena.adjacent_node_ids(index) {
		let amount = *transfer_dirs.get(&Some(adj_index)).unwrap_or(&0.0);
		if amount > 0.0 {
			if turf.total_moles() < amount {
				finalize_eq_neighbors(index, arena, &transfer_dirs, info, eq_movement_graph);
			}
			if let Some(adj_tmix) = arena.get(adj_index) {
				if let Some(amt) = eq_movement_graph.edge_weight_mut(Some(adj_index), Some(index)) {
					*amt = 0.0;
				}
				if turf.mix != adj_tmix.mix {
					drop(GasArena::with_gas_mixtures_mut(
						turf.mix,
						adj_tmix.mix,
						|air, other_air| {
							other_air.merge(&air.remove(amount));
							Ok(())
						},
					));
				}
				let adj_turf_id = adj_tmix.id;
				drop(sender.try_send(Box::new(move || {
					let real_amount = Value::from(amount);
					let turf = unsafe { Value::turf_by_id_unchecked(cur_turf_id) };
					let other_turf = unsafe { Value::turf_by_id_unchecked(adj_turf_id) };
					if let Err(e) =
						turf.call("consider_pressure_difference", &[&other_turf, &real_amount])
					{
						Proc::find(byond_string!("/proc/stack_trace"))
							.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
							.call(&[&Value::from_string(e.message.as_str())?])?;
					}
					Ok(Value::null())
				})));
			}
		}
	}
}

fn finalize_eq_neighbors(
	index: NodeIndex,
	arena: &RwLockReadGuard<TurfGases>,
	transfer_dirs: &HashMap<Option<NodeIndex>, f32, FxBuildHasher>,
	info: &HashMap<NodeIndex, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex>, f32>,
) {
	for adj_index in arena.adjacent_node_ids(index) {
		let amount = *transfer_dirs.get(&Some(adj_index)).unwrap_or(&0.0);
		if amount < 0.0 {
			let other_turf = {
				let maybe = arena.get(adj_index);
				if maybe.is_none() {
					continue;
				}
				maybe.unwrap()
			};
			finalize_eq(adj_index, other_turf, arena, info, eq_movement_graph);
		}
	}
}

fn monstermos_fast_process(
	cur_index: NodeIndex,
	arena: &RwLockReadGuard<TurfGases>,
	info: &mut HashMap<NodeIndex, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex>, f32>,
) {
	let mut cur_info = {
		let mut cur_info = info.get_mut(&cur_index).unwrap();
		cur_info.fast_done = true;
		*cur_info
	};
	let mut eligible_adjacents: Vec<NodeIndex> = Default::default();
	if cur_info.mole_delta > 0.0 {
		for adj_turf in arena.adjacent_node_ids_enabled(cur_index) {
			if let Some(adj_info) = info.get(&adj_turf) {
				if !adj_info.fast_done {
					eligible_adjacents.push(adj_turf);
				}
			}
		}
		if eligible_adjacents.is_empty() {
			info.entry(cur_index).and_modify(|entry| *entry = cur_info);
			return;
		}
		let moles_to_move = cur_info.mole_delta / eligible_adjacents.len() as f32;
		eligible_adjacents.into_iter().for_each(|adj_index| {
			if let Some(mut adj_info) = info.get_mut(&adj_index) {
				adjust_eq_movement(
					Some(cur_index),
					Some(adj_index),
					moles_to_move,
					eq_movement_graph,
				);
				cur_info.mole_delta -= moles_to_move;
				adj_info.mole_delta += moles_to_move;
			}
			info.entry(cur_index).and_modify(|entry| *entry = cur_info);
		});
	}
}

fn give_to_takers(
	giver_turfs: &[RefMixWithID],
	arena: &RwLockReadGuard<TurfGases>,
	info: &mut HashMap<NodeIndex, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex>, f32>,
) {
	let mut queue: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
	for (&index, _) in giver_turfs {
		let mut giver_info = {
			let giver_info = info.get_mut(&index).unwrap();
			giver_info.curr_transfer_dir = None;
			giver_info.curr_transfer_amount = 0.0;
			*giver_info
		};
		queue.insert(index);
		let mut queue_idx = 0;

		while let Some(&cur_index) = queue.get_index(queue_idx) {
			if giver_info.mole_delta <= 0.0 {
				break;
			}
			for adj_idx in arena.adjacent_node_ids_enabled(cur_index) {
				if giver_info.mole_delta <= 0.0 {
					break;
				}
				if let Some(mut adj_info) = info.get_mut(&adj_idx) {
					if queue.insert(adj_idx) {
						adj_info.curr_transfer_dir = Some(cur_index);
						adj_info.curr_transfer_amount = 0.0;
						if adj_info.mole_delta < 0.0 {
							// this turf needs gas. Let's give it to 'em.
							if -adj_info.mole_delta > giver_info.mole_delta {
								// we don't have enough gas
								adj_info.curr_transfer_amount -= giver_info.mole_delta;
								adj_info.mole_delta += giver_info.mole_delta;
								giver_info.mole_delta = 0.0;
							} else {
								// we have enough gas.
								adj_info.curr_transfer_amount += adj_info.mole_delta;
								giver_info.mole_delta += adj_info.mole_delta;
								adj_info.mole_delta = 0.0;
							}
						}
					}
				}
				info.entry(index).and_modify(|entry| *entry = giver_info);
			}

			queue_idx += 1;
		}

		for cur_index in queue.drain(..).rev() {
			let mut turf_info = {
				let opt = info.get(&cur_index);
				if opt.is_none() {
					continue;
				}
				*opt.unwrap()
			};
			if turf_info.curr_transfer_amount != 0.0 && turf_info.curr_transfer_dir.is_some() {
				if let Some(mut adj_info) = info.get_mut(&turf_info.curr_transfer_dir.unwrap()) {
					adjust_eq_movement(
						Some(cur_index),
						turf_info.curr_transfer_dir,
						turf_info.curr_transfer_amount,
						eq_movement_graph,
					);
					adj_info.curr_transfer_amount += turf_info.curr_transfer_amount;
					turf_info.curr_transfer_amount = 0.0;
				}
			}
			info.entry(cur_index)
				.and_modify(|cur_info| *cur_info = turf_info);
		}
	}
}

fn take_from_givers(
	taker_turfs: &[RefMixWithID],
	arena: &RwLockReadGuard<TurfGases>,
	info: &mut HashMap<NodeIndex, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex>, f32>,
) {
	let mut queue: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
	for (&index, _) in taker_turfs {
		let mut taker_info = {
			let taker_info = info.get_mut(&index).unwrap();
			taker_info.curr_transfer_dir = None;
			taker_info.curr_transfer_amount = 0.0;
			*taker_info
		};
		queue.insert(index);
		let mut queue_idx = 0;
		while let Some(&cur_index) = queue.get_index(queue_idx) {
			if taker_info.mole_delta >= 0.0 {
				break;
			}
			for adj_index in arena.adjacent_node_ids_enabled(cur_index) {
				if taker_info.mole_delta >= 0.0 {
					break;
				}
				if let Some(mut adj_info) = info.get_mut(&adj_index) {
					if queue.insert(adj_index) {
						adj_info.curr_transfer_dir = Some(cur_index);
						adj_info.curr_transfer_amount = 0.0;
						if adj_info.mole_delta > 0.0 {
							// this turf has gas we can succ. Time to succ.
							if adj_info.mole_delta > -taker_info.mole_delta {
								// they have enough gase
								adj_info.curr_transfer_amount -= taker_info.mole_delta;
								adj_info.mole_delta += taker_info.mole_delta;
								taker_info.mole_delta = 0.0;
							} else {
								// they don't have neough gas
								adj_info.curr_transfer_amount += adj_info.mole_delta;
								taker_info.mole_delta += adj_info.mole_delta;
								adj_info.mole_delta = 0.0;
							}
						}
					}
				}
				info.entry(index).and_modify(|entry| *entry = taker_info);
			}
			queue_idx += 1;
		}
		for cur_index in queue.drain(..).rev() {
			let mut turf_info = {
				let opt = info.get(&cur_index);
				if opt.is_none() {
					continue;
				}
				*opt.unwrap()
			};
			if turf_info.curr_transfer_amount != 0.0 && turf_info.curr_transfer_dir.is_some() {
				if let Some(mut adj_info) = info.get_mut(&turf_info.curr_transfer_dir.unwrap()) {
					adjust_eq_movement(
						Some(cur_index),
						turf_info.curr_transfer_dir,
						turf_info.curr_transfer_amount,
						eq_movement_graph,
					);
					adj_info.curr_transfer_amount += turf_info.curr_transfer_amount;
					turf_info.curr_transfer_amount = 0.0;
				}
			}
			info.entry(cur_index)
				.and_modify(|cur_info| *cur_info = turf_info);
		}
	}
}

#[inline(never)]
fn explosively_depressurize(initial_index: NodeIndex, equalize_hard_turf_limit: usize) -> DMResult {
	//1st floodfill
	let (space_turfs, warned_about_planet_atmos) = with_turf_gases_read(
		|arena| -> Result<(IndexSet<NodeIndex, FxBuildHasher>, bool), Runtime> {
			let mut cur_queue_idx = 0;
			let mut warned_about_planet_atmos = false;
			let mut space_turfs: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
			let mut turfs: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
			turfs.insert(initial_index);
			while cur_queue_idx < turfs.len() {
				let cur_index = turfs[cur_queue_idx];
				cur_queue_idx += 1;
				let cur_mixture = {
					let maybe = arena.get(cur_index);
					if maybe.is_none() {
						continue;
					}
					*maybe.unwrap()
				};
				if cur_mixture.planetary_atmos.is_some() {
					warned_about_planet_atmos = true;
					continue;
				}
				if cur_mixture.is_immutable() {
					if space_turfs.insert(cur_index) {
						unsafe { Value::turf_by_id_unchecked(cur_mixture.id) }
							.set(byond_string!("pressure_specific_target"), &unsafe {
								Value::turf_by_id_unchecked(cur_mixture.id)
							})?;
					}
				} else {
					if cur_queue_idx > equalize_hard_turf_limit {
						continue;
					}
					for (adj_index, adj_mixture) in arena
						.adjacent_node_ids(cur_index)
						.filter_map(|index| Some((index, arena.get(index)?)))
					{
						if turfs.insert(adj_index) {
							unsafe { Value::turf_by_id_unchecked(cur_mixture.id) }.call(
								"consider_firelocks",
								&[&unsafe { Value::turf_by_id_unchecked(adj_mixture.id) }],
							)?;
						}
					}
				}
				if warned_about_planet_atmos {
					break;
				}
			}
			Ok((space_turfs, warned_about_planet_atmos))
		},
	)?;

	if warned_about_planet_atmos {
		return Ok(Value::null()); // planet atmos > space
	}
	if space_turfs.is_empty() {
		return Ok(Value::null());
	}

	//actually update the damn arena to register the firelocks closing
	process_aux_callbacks(crate::callbacks::TURFS);
	process_aux_callbacks(crate::callbacks::ADJACENCIES);

	with_turf_gases_read(move |arena| {
		let mut info: HashMap<NodeIndex, Cell<ReducedInfo>, FxBuildHasher> = Default::default();
		let mut progression_order: IndexSet<MixWithID, RandomState> = Default::default();
		for &cur_index in space_turfs.iter() {
			let maybe_turf = arena.get(cur_index);
			if maybe_turf.is_none() {
				continue;
			}
			let cur_mixture = maybe_turf.unwrap();
			progression_order.insert((cur_index, *cur_mixture));
		}

		let mut space_turf_len = 0;
		let mut total_moles = 0.0;
		let mut cur_queue_idx = 0;
		//2nd floodfill
		while cur_queue_idx < progression_order.len() {
			let (cur_index, cur_mixture) = progression_order[cur_queue_idx];
			cur_queue_idx += 1;

			total_moles += cur_mixture.total_moles();
			cur_mixture.is_immutable().then(|| space_turf_len += 1);

			if cur_queue_idx > equalize_hard_turf_limit {
				continue;
			}

			for adj_turf in arena.adjacent_node_ids(cur_index) {
				if let Some(adj_mixture) = arena.get(adj_turf) {
					let adj_orig = info.entry(adj_turf).or_default();
					let mut adj_info = adj_orig.get();
					if !adj_mixture.is_immutable()
						&& progression_order.insert((adj_turf, *adj_mixture))
					{
						adj_info.curr_transfer_dir = Some(cur_index);
						adj_info.curr_transfer_amount = 0.0;
						let cur_target_turf =
							unsafe { Value::turf_by_id_unchecked(cur_mixture.id) }
								.get(byond_string!("pressure_specific_target"))?;
						unsafe { Value::turf_by_id_unchecked(adj_mixture.id) }
							.set(byond_string!("pressure_specific_target"), &cur_target_turf)?;
						adj_orig.set(adj_info);
					}
				}
			}
		}

		let _average_moles = total_moles / (progression_order.len() - space_turf_len) as f32;

		let hpd = auxtools::Value::globals()
			.get(byond_string!("SSair"))?
			.get_list(byond_string!("high_pressure_delta"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-list value as list {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?;

		let get_dir = Proc::find(byond_string!("/proc/get_dir_multiz")).map_or(
			Err(runtime!(
				"Proc get_dir_multiz not found! {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)),
			|opt| Ok(opt),
		)?;

		for (cur_index, cur_mixture) in progression_order.iter().rev() {
			let cur_orig = info.entry(*cur_index).or_default();
			let mut cur_info = cur_orig.get();
			if cur_info.curr_transfer_dir.is_none() {
				continue;
			}
			let mut in_hpd = false;
			for k in 1..=hpd.len() {
				if let Ok(hpd_val) = hpd.get(k) {
					if hpd_val == unsafe { Value::turf_by_id_unchecked(cur_mixture.id) } {
						in_hpd = true;
						break;
					}
				}
			}
			if !in_hpd {
				hpd.append(&unsafe { Value::turf_by_id_unchecked(cur_mixture.id) });
			}
			let adj_index = cur_info.curr_transfer_dir.unwrap();

			let adj_mixture = arena.get(adj_index).unwrap();
			let sum = adj_mixture.total_moles();

			cur_info.curr_transfer_amount += sum;
			cur_orig.set(cur_info);

			let adj_orig = info.entry(adj_index).or_default();
			let mut adj_info = adj_orig.get();

			adj_info.curr_transfer_amount += cur_info.curr_transfer_amount;
			adj_orig.set(adj_info);

			let byond_turf = unsafe { Value::turf_by_id_unchecked(cur_mixture.id) };
			let byond_turf_adj = unsafe { Value::turf_by_id_unchecked(adj_mixture.id) };

			byond_turf.set(
				byond_string!("pressure_difference"),
				Value::from(cur_info.curr_transfer_amount),
			)?;
			byond_turf.set(
				byond_string!("pressure_direction"),
				get_dir.call(&[&byond_turf, &byond_turf_adj])?,
			)?;

			if adj_info.curr_transfer_dir.is_none() {
				byond_turf_adj.set(
					byond_string!("pressure_difference"),
					Value::from(adj_info.curr_transfer_amount),
				)?;
				byond_turf_adj.set(
					byond_string!("pressure_direction"),
					get_dir.call(&[&byond_turf_adj, &byond_turf])?,
				)?;
			}

			#[cfg(not(feature = "katmos_slow_decompression"))]
			{
				cur_mixture.clear_air();
			}
			#[cfg(feature = "katmos_slow_decompression")]
			{
				const DECOMP_REMOVE_RATIO: f32 = 4_f32;
				cur_mixture.clear_vol((_average_moles / DECOMP_REMOVE_RATIO).abs());
			}

			byond_turf.call("handle_decompression_floor_rip", &[&Value::from(sum)])?;
		}
		Ok(())
	})?;

	Ok(Value::null())
}

// Clippy go away, this type is only used once
#[allow(clippy::type_complexity)]
fn flood_fill_equalize_turfs(
	index: NodeIndex,
	equalize_hard_turf_limit: usize,
	found_turfs: &mut HashSet<NodeIndex, FxBuildHasher>,
	arena: &RwLockReadGuard<TurfGases>,
) -> Option<(
	IndexMap<NodeIndex, TurfMixture, FxBuildHasher>,
	IndexSet<NodeIndex, FxBuildHasher>,
	f64,
)> {
	let mut turfs: IndexMap<NodeIndex, TurfMixture, FxBuildHasher> = Default::default();
	let mut border_turfs: std::collections::VecDeque<(NodeIndex, TurfMixture)> = Default::default();
	let mut planet_turfs: IndexSet<NodeIndex, FxBuildHasher> = Default::default();
	let sender = byond_callback_sender();
	let mut total_moles = 0.0_f64;
	border_turfs.push_back((index, *arena.get(index).unwrap()));
	found_turfs.insert(index);
	let mut ignore_zone = false;
	while let Some((cur_index, cur_mixture)) = border_turfs.pop_front() {
		let cur_turf = arena.get(cur_index).unwrap();
		if cur_turf.planetary_atmos.is_some() {
			planet_turfs.insert(cur_index);
			continue;
		}
		total_moles += cur_turf.total_moles() as f64;

		for adj_index in arena.adjacent_node_ids(cur_index) {
			if found_turfs.insert(adj_index) {
				if let Some(adj_mixture) = arena.get(adj_index) {
					if adj_mixture.enabled() {
						border_turfs.push_back((adj_index, *adj_mixture));
					}
					if adj_mixture.is_immutable() {
						// Uh oh! looks like someone opened an airlock to space! TIME TO SUCK ALL THE AIR OUT!!!
						// NOT ONE OF YOU IS GONNA SURVIVE THIS
						// (I just made explosions less laggy, you're welcome)
						if !ignore_zone {
							drop(sender.send(Box::new(move || {
								explosively_depressurize(adj_index, equalize_hard_turf_limit)
							})));
						}
						ignore_zone = true;
					}
				}
			}
		}
		turfs.insert(cur_index, cur_mixture);
	}
	(!ignore_zone).then(|| (turfs, planet_turfs, total_moles))
}

fn process_planet_turfs(
	planet_turfs: &IndexSet<NodeIndex, FxBuildHasher>,
	arena: &RwLockReadGuard<TurfGases>,
	average_moles: f32,
	equalize_hard_turf_limit: usize,
	info: &mut HashMap<NodeIndex, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex>, f32>,
) {
	let sender = byond_callback_sender();
	let sample_turf = arena.get(planet_turfs[0]);
	if sample_turf.is_none() {
		return;
	}
	let sample_turf = sample_turf.unwrap();
	let sample_planet_atmos = sample_turf.planetary_atmos;
	if sample_planet_atmos.is_none() {
		return;
	}
	let maybe_planet_sum = planetary_atmos()
		.try_get(&sample_planet_atmos.unwrap())
		.try_unwrap();
	if maybe_planet_sum.is_none() {
		return;
	}
	let planet_sum = maybe_planet_sum.unwrap().value().total_moles();
	let target_delta = planet_sum - average_moles;

	let mut progression_order: IndexSet<NodeIndex, FxBuildHasher> = Default::default();

	for cur_index in planet_turfs.iter() {
		progression_order.insert(*cur_index);
		let mut cur_info = info.entry(*cur_index).or_default();
		cur_info.curr_transfer_dir = None;
	}
	// now build a map of where the path to a planet turf is for each tile.
	let mut queue_idx = 0;
	while queue_idx < progression_order.len() {
		let cur_index = progression_order[queue_idx];
		queue_idx += 1;
		let maybe_m = arena.get(cur_index);
		if maybe_m.is_none() {
			info.entry(cur_index)
				.and_modify(|entry| *entry = MonstermosInfo::default());
			continue;
		}
		let cur_mixture_id = maybe_m.unwrap().id;
		for adj_index in arena.adjacent_node_ids(cur_index) {
			if let Some(mut adj_info) = info.get_mut(&adj_index) {
				let adj_mixture_id = arena.get(adj_index).unwrap().id;
				if queue_idx < equalize_hard_turf_limit {
					drop(sender.try_send(Box::new(move || {
						let this_turf = unsafe { Value::turf_by_id_unchecked(cur_mixture_id) };
						this_turf.call(
							"consider_firelocks",
							&[&unsafe { Value::turf_by_id_unchecked(adj_mixture_id) }],
						)?;
						Ok(Value::null())
					})));
				}
				if let Some(adj_mixture) = arena
					.get(adj_index)
					.and_then(|terf| terf.enabled().then(|| terf))
				{
					if !progression_order.insert(adj_index) || adj_mixture.planetary_atmos.is_some()
					{
						continue;
					}
					adj_info.curr_transfer_dir = Some(cur_index);
				}
			}
		}
	}
	for &cur_index in progression_order.iter().rev() {
		if arena.get(cur_index).is_none() {
			continue;
		}
		let mut cur_info = {
			if let Some(opt) = info.get(&cur_index) {
				*opt
			} else {
				continue;
			}
		};
		let airflow = cur_info.mole_delta - target_delta;
		if cur_info.curr_transfer_dir.is_none() {
			adjust_eq_movement(Some(cur_index), None, airflow, eq_movement_graph);
			cur_info.mole_delta = target_delta;
		} else if let Some(mut adj_info) = info.get_mut(&cur_info.curr_transfer_dir.unwrap()) {
			adjust_eq_movement(
				Some(cur_index),
				cur_info.curr_transfer_dir,
				airflow,
				eq_movement_graph,
			);
			adj_info.mole_delta += airflow;
			cur_info.mole_delta = target_delta;
		}
		info.entry(cur_index).and_modify(|info| *info = cur_info);
	}
}

pub(crate) fn equalize(
	equalize_hard_turf_limit: usize,
	high_pressure_turfs: &[NodeIndex],
) -> usize {
	let turfs_processed: AtomicUsize = AtomicUsize::new(0);
	let mut found_turfs: HashSet<NodeIndex, FxBuildHasher> = Default::default();
	with_turf_gases_read(|arena| {
		let zoned_turfs = high_pressure_turfs
			.iter()
			.filter_map(|&cur_index| {
				//is this turf already visited?
				if found_turfs.contains(&cur_index) {
					return None;
				};

				//is this turf exists/enabled/have adjacencies?
				let cur_mixture = *arena.get(cur_index)?;
				if !cur_mixture.enabled() || arena.adjacent_node_ids(cur_index).next().is_none() {
					return None;
				}

				//does this turf or its adjacencies have enough moles to share?
				if GasArena::with_all_mixtures(|all_mixtures| {
					let our_moles = all_mixtures[cur_mixture.mix].read().total_moles();
					our_moles < 10.0
						|| arena.adjacent_mixes(cur_index, all_mixtures).all(|lock| {
							(lock.read().total_moles() - our_moles).abs()
								< MINIMUM_MOLES_DELTA_TO_MOVE
						})
				}) {
					return None;
				}

				flood_fill_equalize_turfs(
					cur_index,
					equalize_hard_turf_limit,
					&mut found_turfs,
					&arena,
				)
			})
			.collect::<Vec<_>>();

		let turfs = zoned_turfs
			.into_par_iter()
			.map(|(turfs, planet_turfs, total_moles)| {
				let mut graph: DiGraphMap<Option<NodeIndex>, f32> = Default::default();
				let average_moles =
					(total_moles / (turfs.len() - planet_turfs.len()) as f64) as f32;

				let mut info = turfs
					.par_iter()
					.map(|(&index, mixture)| {
						let mut cur_info = MonstermosInfo::default();
						cur_info.mole_delta = mixture.total_moles() - average_moles;
						(index, cur_info)
					})
					.collect::<HashMap<_, _, FxBuildHasher>>();

				let (mut giver_turfs, mut taker_turfs): (Vec<_>, Vec<_>) = turfs
					.iter()
					.filter(|(_, cur_mixture)| cur_mixture.planetary_atmos.is_none())
					.partition(|(i, _)| info.get(i).unwrap().mole_delta > 0.0);

				let log_n = ((turfs.len() as f32).log2().floor()) as usize;
				if giver_turfs.len() > log_n && taker_turfs.len() > log_n {
					for (&cur_index, _) in &turfs {
						monstermos_fast_process(cur_index, &arena, &mut info, &mut graph);
					}

					giver_turfs.clear();
					taker_turfs.clear();

					giver_turfs.extend(turfs.iter().filter(|(cur_index, cur_mixture)| {
						info.get(cur_index).unwrap().mole_delta > 0.0
							&& cur_mixture.planetary_atmos.is_none()
					}));

					taker_turfs.extend(turfs.iter().filter(|(cur_index, cur_mixture)| {
						info.get(cur_index).unwrap().mole_delta <= 0.0
							&& cur_mixture.planetary_atmos.is_none()
					}));
				}

				// alright this is the part that can become O(n^2).
				if giver_turfs.len() < taker_turfs.len() {
					// as an optimization, we choose one of two methods based on which list is smaller.
					give_to_takers(&giver_turfs, &arena, &mut info, &mut graph);
				} else {
					take_from_givers(&taker_turfs, &arena, &mut info, &mut graph);
				}
				if planet_turfs.is_empty() {
					turfs_processed.fetch_add(turfs.len(), std::sync::atomic::Ordering::Relaxed);
				} else {
					turfs_processed.fetch_add(
						turfs.len() + planet_turfs.len(),
						std::sync::atomic::Ordering::Relaxed,
					);
					process_planet_turfs(
						&planet_turfs,
						&arena,
						average_moles,
						equalize_hard_turf_limit,
						&mut info,
						&mut graph,
					);
				}
				(turfs, info, graph)
			})
			.collect::<Vec<_>>();

		turfs.into_par_iter().for_each(|(turf, info, mut graph)| {
			turf.iter().for_each(|(&cur_index, cur_mixture)| {
				finalize_eq(cur_index, cur_mixture, &arena, &info, &mut graph);
			});
		});
	});
	turfs_processed.load(std::sync::atomic::Ordering::Relaxed)
}
