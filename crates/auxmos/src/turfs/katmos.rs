//Monstermos, but zoned, and multithreaded!

use super::*;

use indexmap::IndexSet;

use ahash::RandomState;

use fxhash::FxBuildHasher;

use auxcallback::byond_callback_sender;

use petgraph::{graph::NodeIndex, graphmap::DiGraphMap};

use std::{
	cell::Cell,
	{
		collections::{HashMap, HashSet},
		sync::atomic::{AtomicBool, AtomicUsize, Ordering},
	},
};

#[derive(Copy, Clone)]
struct MonstermosInfo {
	mole_delta: f32,
	curr_transfer_amount: f32,
	curr_transfer_dir: Option<NodeIndex<usize>>,
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
	curr_transfer_dir: Option<NodeIndex<usize>>,
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
	this_turf: Option<NodeIndex<usize>>,
	that_turf: Option<NodeIndex<usize>>,
	amount: f32,
	graph: &mut DiGraphMap<Option<NodeIndex<usize>>, f32>,
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
	index: NodeIndex<usize>,
	arena: &TurfGases,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex<usize>>, f32>,
	pressures: &mut Vec<(f32, u32, u32)>,
) {
	let turf = arena.get(index).unwrap();
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
				finalize_eq_neighbors(index, arena, &transfer_dirs, eq_movement_graph, pressures);
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
				finalize_eq_neighbors(index, arena, &transfer_dirs, eq_movement_graph, pressures);
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
				pressures.push((amount, cur_turf_id, adj_turf_id));
			}
		}
	}
}

fn finalize_eq_neighbors(
	index: NodeIndex<usize>,
	arena: &TurfGases,
	transfer_dirs: &HashMap<Option<NodeIndex<usize>>, f32, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex<usize>>, f32>,
	pressures: &mut Vec<(f32, u32, u32)>,
) {
	for adj_index in arena.adjacent_node_ids(index) {
		let amount = *transfer_dirs.get(&Some(adj_index)).unwrap_or(&0.0);
		if amount < 0.0 {
			finalize_eq(adj_index, arena, eq_movement_graph, pressures);
		}
	}
}

fn monstermos_fast_process(
	cur_index: NodeIndex<usize>,
	arena: &TurfGases,
	info: &mut HashMap<NodeIndex<usize>, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex<usize>>, f32>,
) {
	let mut cur_info = {
		let mut cur_info = info.get_mut(&cur_index).unwrap();
		cur_info.fast_done = true;
		*cur_info
	};
	let mut eligible_adjacents: Vec<NodeIndex<usize>> = Default::default();
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
	giver_turfs: &[NodeIndex<usize>],
	arena: &TurfGases,
	info: &mut HashMap<NodeIndex<usize>, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex<usize>>, f32>,
) {
	let mut queue: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
	for &index in giver_turfs {
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
	taker_turfs: &[NodeIndex<usize>],
	arena: &TurfGases,
	info: &mut HashMap<NodeIndex<usize>, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex<usize>>, f32>,
) {
	let mut queue: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
	for &index in taker_turfs {
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

fn explosively_depressurize(
	initial_index: NodeIndex<usize>,
	equalize_hard_turf_limit: usize,
) -> Result<(), Runtime> {
	//1st floodfill
	let (space_turfs, warned_about_planet_atmos) = {
		let mut cur_queue_idx = 0;
		let mut warned_about_planet_atmos = false;
		let mut space_turfs: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
		let mut turfs: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
		turfs.insert(initial_index);
		while cur_queue_idx < turfs.len() {
			let cur_index = turfs[cur_queue_idx];
			let mut had_firelock = false;
			cur_queue_idx += 1;
			with_turf_gases_read(|arena| -> Result<(), Runtime> {
				let cur_mixture = {
					let maybe = arena.get(cur_index);
					if maybe.is_none() {
						return Ok(());
					}
					maybe.unwrap()
				};
				if cur_mixture.planetary_atmos.is_some() {
					warned_about_planet_atmos = true;
					return Ok(());
				}
				if cur_mixture.is_immutable() {
					if space_turfs.insert(cur_index) {
						unsafe { Value::turf_by_id_unchecked(cur_mixture.id) }
							.set(byond_string!("pressure_specific_target"), &unsafe {
								Value::turf_by_id_unchecked(cur_mixture.id)
							})?;
					}
				} else if cur_mixture.enabled() {
					if cur_queue_idx > equalize_hard_turf_limit {
						return Ok(());
					}
					for (flags, adj_index, adj_mixture) in
						arena.graph.edges(cur_index).filter_map(|edge| {
							Some((edge.weight(), edge.target(), arena.get(edge.target())?))
						}) {
						if turfs.insert(adj_index)
							&& flags.contains(AdjacentFlags::ATMOS_ADJACENT_FIRELOCK)
						{
							had_firelock = true;
							unsafe { Value::turf_by_id_unchecked(cur_mixture.id) }.call(
								"consider_firelocks",
								&[&unsafe { Value::turf_by_id_unchecked(adj_mixture.id) }],
							)?;
						}
					}
				}
				Ok(())
			})?;
			if had_firelock {
				rebuild_turf_graph()?; // consider_firelocks ought to dirtify it anyway
			}
			if warned_about_planet_atmos {
				break;
			}
		}
		(space_turfs, warned_about_planet_atmos)
	};

	if warned_about_planet_atmos || space_turfs.is_empty() {
		return Ok(()); // planet atmos > space
	}

	with_turf_gases_read(move |arena| {
		let mut info: HashMap<NodeIndex<usize>, Cell<ReducedInfo>, FxBuildHasher> =
			Default::default();
		let mut progression_order: IndexSet<NodeIndex<usize>, RandomState> = Default::default();
		for &cur_index in space_turfs.iter() {
			let maybe_turf = arena.get(cur_index);
			if maybe_turf.is_none() {
				continue;
			}
			progression_order.insert(cur_index);
		}

		let mut space_turf_len = 0;
		let mut total_moles = 0.0;
		let mut cur_queue_idx = 0;
		//2nd floodfill
		while cur_queue_idx < progression_order.len() {
			let cur_index = progression_order[cur_queue_idx];
			let cur_mixture = arena.get(cur_index).unwrap();
			cur_queue_idx += 1;

			total_moles += cur_mixture.total_moles();
			cur_mixture.is_immutable().then(|| space_turf_len += 1);

			if cur_queue_idx > equalize_hard_turf_limit {
				continue;
			}

			for adj_index in arena.adjacent_node_ids(cur_index) {
				if let Some(adj_mixture) = arena.get(adj_index) {
					let adj_orig = info.entry(adj_index).or_default();
					let mut adj_info = adj_orig.get();
					if !adj_mixture.is_immutable() && progression_order.insert(adj_index) {
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
			Ok,
		)?;

		for &cur_index in progression_order.iter().rev() {
			let cur_orig = info.entry(cur_index).or_default();
			let cur_mixture = arena.get(cur_index).unwrap();
			let mut cur_info = cur_orig.get();
			if cur_info.curr_transfer_dir.is_none() {
				continue;
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

			byond_turf.call("handle_decompression_floor_rip", &[&Value::from(sum)])?;
		}
		Ok(())
	})?;

	Ok(())
}

// Clippy go away, this type is only used once
#[allow(clippy::type_complexity)]
fn flood_fill_equalize_turfs(
	index: NodeIndex<usize>,
	equalize_hard_turf_limit: usize,
	found_turfs: &mut HashSet<NodeIndex<usize>, FxBuildHasher>,
	arena: &TurfGases,
	contains_planet: &AtomicBool,
) -> Option<(
	IndexSet<NodeIndex<usize>, FxBuildHasher>,
	IndexSet<NodeIndex<usize>, FxBuildHasher>,
	f32,
)> {
	let mut turfs: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
	let mut border_turfs: std::collections::VecDeque<NodeIndex<usize>> = Default::default();
	let mut planet_turfs: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
	let sender = byond_callback_sender();
	let mut total_moles = 0.0_f32;
	border_turfs.push_back(index);
	found_turfs.insert(index);
	let mut ignore_zone = false;
	while let Some(cur_index) = border_turfs.pop_front() {
		let cur_turf = arena.get(cur_index).unwrap();
		if cur_turf.planetary_atmos.is_some() {
			let _ =
				contains_planet.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed);
			planet_turfs.insert(cur_index);
			continue;
		}
		total_moles += cur_turf.total_moles();

		for adj_index in arena.adjacent_node_ids(cur_index) {
			if found_turfs.insert(adj_index) {
				if let Some(adj_mixture) = arena.get(adj_index) {
					if adj_mixture.enabled() {
						border_turfs.push_back(adj_index);
					}
					if adj_mixture.is_immutable() {
						// Uh oh! looks like someone opened an airlock to space! TIME TO SUCK ALL THE AIR OUT!!!
						// NOT ONE OF YOU IS GONNA SURVIVE THIS
						// (I just made explosions less laggy, you're welcome)
						if !ignore_zone {
							drop(sender.try_send(Box::new(move || {
								explosively_depressurize(cur_index, equalize_hard_turf_limit)
							})));
						}
						ignore_zone = true;
					}
				}
			}
		}
		turfs.insert(cur_index);
	}
	(!ignore_zone).then(|| (turfs, planet_turfs, total_moles))
}

fn process_planet_turfs(
	planet_turfs: &IndexSet<NodeIndex<usize>, FxBuildHasher>,
	arena: &TurfGases,
	average_moles: f32,
	did_firelocks: bool,
	equalize_hard_turf_limit: usize,
	info: &mut HashMap<NodeIndex<usize>, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<Option<NodeIndex<usize>>, f32>,
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

	let mut progression_order: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();

	for &cur_index in planet_turfs {
		progression_order.insert(cur_index);
		let mut cur_info = info.entry(cur_index).or_default();
		cur_info.curr_transfer_dir = None;
	}
	// now build a map of where the path to a planet turf is for each tile.
	let mut queue_idx = 0;
	let mut firelock_callbacks = Vec::new();
	while queue_idx < progression_order.len() {
		let &cur_index = progression_order.get_index(queue_idx).unwrap();
		let cur_mixture = arena.get(cur_index).unwrap();
		queue_idx += 1;
		for (flags, adj_index) in arena
			.graph
			.edges(cur_index)
			.map(|edge| (edge.weight(), edge.target()))
		{
			if let Some(mut adj_info) = info.get_mut(&adj_index) {
				let adj_mixture_id = arena.get(adj_index).unwrap().id;
				if flags.contains(AdjacentFlags::ATMOS_ADJACENT_FIRELOCK)
					&& queue_idx < equalize_hard_turf_limit
					&& !did_firelocks
				{
					let cur_id = cur_mixture.id;
					firelock_callbacks.push(Box::new(move || -> DMResult {
						let this_turf = unsafe { Value::turf_by_id_unchecked(cur_id) };
						this_turf.call(
							"consider_firelocks",
							&[&unsafe { Value::turf_by_id_unchecked(adj_mixture_id) }],
						)?;
						Ok(Value::null())
					}));
				}
				if arena.get(adj_index).map_or(false, |terf| terf.enabled()) {
					if !progression_order.insert(adj_index) {
						continue;
					}
					adj_info.curr_transfer_dir = Some(cur_index);
				}
			}
		}
	}

	//whoops, this can absolutely fail
	if sender
		.try_send(Box::new(move || {
			for callback in firelock_callbacks.iter() {
				callback()?;
			}
			Ok(())
		}))
		.is_err()
	{
		PLANET_TURF_CYCLE.store(false, Ordering::Relaxed);
		return;
	}

	if !did_firelocks {
		return;
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

static PLANET_TURF_CYCLE: AtomicBool = AtomicBool::new(false);

pub fn equalize(
	equalize_hard_turf_limit: usize,
	high_pressure_turfs: &std::collections::BTreeSet<NodeIndex<usize>>,
	planet_enabled: bool,
) -> usize {
	let turfs_processed: AtomicUsize = AtomicUsize::new(0);
	let mut found_turfs: HashSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
	let contains_planet: AtomicBool = AtomicBool::new(false);
	with_turf_gases_read(|arena| {
		let zoned_turfs = high_pressure_turfs
			.iter()
			.filter_map(|&cur_index| {
				//is this turf already visited?
				if found_turfs.contains(&cur_index) {
					return None;
				};

				//is this turf exists/enabled/have adjacencies?
				let cur_mixture = arena.get(cur_index)?;
				if !cur_mixture.enabled() || arena.adjacent_node_ids(cur_index).next().is_none() {
					return None;
				}

				let is_unshareable = GasArena::with_all_mixtures(|all_mixtures| {
					let our_moles = all_mixtures[cur_mixture.mix].read().total_moles();
					our_moles < 10.0
						|| arena.adjacent_mixes(cur_index, all_mixtures).all(|lock| {
							(lock.read().total_moles() - our_moles).abs()
								< MINIMUM_MOLES_DELTA_TO_MOVE
						})
				});

				//does this turf or its adjacencies have enough moles to share?
				if is_unshareable {
					return None;
				}

				flood_fill_equalize_turfs(
					cur_index,
					equalize_hard_turf_limit,
					&mut found_turfs,
					arena,
					&contains_planet,
				)
			})
			.collect::<Vec<_>>();

		if check_turfs_dirty() {
			return;
		}

		let did_firelocks = {
			if contains_planet.load(Ordering::Acquire) {
				PLANET_TURF_CYCLE.fetch_xor(true, Ordering::Relaxed)
			} else {
				PLANET_TURF_CYCLE.store(false, Ordering::Relaxed);
				false
			}
		};

		let turfs = zoned_turfs
			.into_par_iter()
			.map(|(turfs, planet_turfs, total_moles)| {
				let mut graph: DiGraphMap<Option<NodeIndex<usize>>, f32> = Default::default();
				let average_moles = {
					let tlen = turfs.len();
					let plen = planet_turfs.len();
					if plen >= tlen {
						0.0
					} else {
						(total_moles / (turfs.len() - planet_turfs.len()) as f32) as f32
					}
				};

				let mut info = turfs
					.par_iter()
					.map(|&index| {
						let mixture = arena.get(index).unwrap();
						let cur_info = MonstermosInfo {
							mole_delta: mixture.total_moles() - average_moles,
							..Default::default()
						};
						(index, cur_info)
					})
					.collect::<HashMap<_, _, FxBuildHasher>>();

				let (mut giver_turfs, mut taker_turfs): (Vec<_>, Vec<_>) = turfs
					.iter()
					.filter(|&&index| arena.get(index).unwrap().planetary_atmos.is_none())
					.partition(|i| info.get(i).unwrap().mole_delta > 0.0);

				let log_n = ((turfs.len() as f32).log2().floor()) as usize;
				if giver_turfs.len() > log_n && taker_turfs.len() > log_n {
					for &cur_index in &turfs {
						monstermos_fast_process(cur_index, arena, &mut info, &mut graph);
					}

					giver_turfs.clear();
					taker_turfs.clear();

					giver_turfs.extend(turfs.iter().filter(|cur_index| {
						info.get(cur_index).unwrap().mole_delta > 0.0
							&& arena.get(**cur_index).unwrap().planetary_atmos.is_none()
					}));

					taker_turfs.extend(turfs.iter().filter(|cur_index| {
						info.get(cur_index).unwrap().mole_delta <= 0.0
							&& arena.get(**cur_index).unwrap().planetary_atmos.is_none()
					}));
				}

				// alright this is the part that can become O(n^2).
				if giver_turfs.len() < taker_turfs.len() {
					// as an optimization, we choose one of two methods based on which list is smaller.
					give_to_takers(&giver_turfs, arena, &mut info, &mut graph);
				} else {
					take_from_givers(&taker_turfs, arena, &mut info, &mut graph);
				}

				if !planet_turfs.is_empty() && planet_enabled {
					turfs_processed.fetch_add(turfs.len() + planet_turfs.len(), Ordering::Relaxed);
					process_planet_turfs(
						&planet_turfs,
						arena,
						average_moles,
						did_firelocks,
						equalize_hard_turf_limit,
						&mut info,
						&mut graph,
					);
				} else {
					turfs_processed.fetch_add(turfs.len(), Ordering::Relaxed);
				}

				(turfs, graph)
			})
			.collect::<Vec<_>>();

		if check_turfs_dirty() {
			return;
		}

		let final_pressures = turfs
			.into_par_iter()
			.filter_map(|(turf, mut graph)| {
				let mut pressures: Vec<(f32, u32, u32)> = Vec::new();
				turf.iter().for_each(|&cur_index| {
					finalize_eq(cur_index, arena, &mut graph, &mut pressures);
				});
				(pressures.len() > 0).then(|| pressures)
			})
			.collect::<Vec<_>>();

		let sender = byond_callback_sender();

		final_pressures.into_iter().for_each(|final_pressures| {
			for (amt, cur_turf, adj_turf) in final_pressures {
				drop(sender.try_send(Box::new(move || {
					let real_amount = Value::from(amt);
					let turf = unsafe { Value::turf_by_id_unchecked(cur_turf) };
					let other_turf = unsafe { Value::turf_by_id_unchecked(adj_turf) };
					if let Err(e) =
						turf.call("consider_pressure_difference", &[&other_turf, &real_amount])
					{
						Proc::find(byond_string!("/proc/stack_trace"))
							.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
							.call(&[&Value::from_string(e.message.as_str())?])?;
					}
					Ok(())
				})));
			}
		});
	});
	turfs_processed.load(Ordering::Relaxed)
}
