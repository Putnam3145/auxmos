//Monstermos, but zoned, and multithreaded!

use super::*;

use indexmap::IndexSet;

use fxhash::FxBuildHasher;

use auxcallback::byond_callback_sender;

use petgraph::{graph::NodeIndex, graphmap::DiGraphMap};

use std::{
	cell::Cell,
	{
		collections::{HashMap, HashSet},
		sync::atomic::{AtomicUsize, Ordering},
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
	this_turf: NodeIndex<usize>,
	that_turf: NodeIndex<usize>,
	amount: f32,
	graph: &mut DiGraphMap<NodeIndex<usize>, f32>,
) {
	if graph.contains_edge(this_turf, that_turf) {
		*graph.edge_weight_mut(this_turf, that_turf).unwrap() += amount;
	} else {
		graph.add_edge(this_turf, that_turf, amount);
	}
	if graph.contains_edge(that_turf, this_turf) {
		*graph.edge_weight_mut(that_turf, this_turf).unwrap() -= amount;
	} else {
		graph.add_edge(that_turf, this_turf, -amount);
	}
}

fn finalize_eq(
	index: NodeIndex<usize>,
	arena: &TurfGases,
	eq_movement_graph: &mut DiGraphMap<NodeIndex<usize>, f32>,
	pressures: &mut Vec<(f32, u32, u32)>,
) {
	let cur_turf_id = arena.get(index).unwrap().id;
	for adj_index in arena.adjacent_node_ids(index) {
		if let Some(&amount) = eq_movement_graph.edge_weight(index, adj_index) {
			if amount > 0.0 {
				if let Some(adj_tmix) = arena.get(adj_index) {
					if let Some(amt) = eq_movement_graph.edge_weight_mut(adj_index, index) {
						*amt = 0.0;
					}
					let turf = arena.get(index).unwrap();
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
}

fn monstermos_fast_process(
	cur_index: NodeIndex<usize>,
	arena: &TurfGases,
	info: &mut HashMap<NodeIndex<usize>, MonstermosInfo, FxBuildHasher>,
	eq_movement_graph: &mut DiGraphMap<NodeIndex<usize>, f32>,
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
				adjust_eq_movement(cur_index, adj_index, moles_to_move, eq_movement_graph);
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
	eq_movement_graph: &mut DiGraphMap<NodeIndex<usize>, f32>,
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
						cur_index,
						turf_info.curr_transfer_dir.unwrap(),
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
	eq_movement_graph: &mut DiGraphMap<NodeIndex<usize>, f32>,
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
						cur_index,
						turf_info.curr_transfer_dir.unwrap(),
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

		let mut progression_order = space_turfs
			.iter()
			.filter_map(|item| arena.get(*item).map_or_else(|| None, |_| Some(*item)))
			.collect::<IndexSet<_, FxBuildHasher>>();

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
fn flood_fill_zones(
	index: NodeIndex<usize>,
	equalize_hard_turf_limit: usize,
	found_turfs: &mut HashSet<NodeIndex<usize>, FxBuildHasher>,
	arena: &TurfGases,
	planet_enabled: bool,
) -> Option<(IndexSet<NodeIndex<usize>, FxBuildHasher>, f32)> {
	let mut turfs: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
	let mut border_turfs: std::collections::VecDeque<NodeIndex<usize>> = Default::default();
	let sender = byond_callback_sender();
	let mut total_moles = 0.0_f32;
	border_turfs.push_back(index);
	found_turfs.insert(index);
	let mut ignore_decomp = false;
	let mut ignore_planet = false;
	while let Some(cur_index) = border_turfs.pop_front() {
		let cur_turf = arena.get(cur_index).unwrap();

		total_moles += cur_turf.total_moles();

		for adj_index in arena.adjacent_node_ids(cur_index) {
			if found_turfs.insert(adj_index) {
				if let Some(adj_mixture) = arena.get(adj_index) {
					if adj_mixture.enabled() && adj_mixture.planetary_atmos.is_none() {
						border_turfs.push_back(adj_index);
					}

					if ignore_decomp || ignore_planet {
						continue;
					}

					if adj_mixture.is_immutable() {
						// Uh oh! looks like someone opened an airlock to space! TIME TO SUCK ALL THE AIR OUT!!!
						// NOT ONE OF YOU IS GONNA SURVIVE THIS
						// (I just made explosions less laggy, you're welcome)
						drop(sender.try_send(Box::new(move || {
							explosively_depressurize(cur_index, equalize_hard_turf_limit)
						})));
						ignore_decomp = true;
					}

					if planet_enabled && adj_mixture.planetary_atmos.is_some() {
						drop(sender.try_send(Box::new(move || {
							planet_equalize(adj_index, equalize_hard_turf_limit)
						})));
						ignore_planet = true;
					}
				}
			}
		}
		turfs.insert(cur_index);
	}
	(!ignore_decomp).then_some((turfs, total_moles))
}

fn planet_equalize(
	initial_index: NodeIndex<usize>,
	equalize_hard_turf_limit: usize,
) -> Result<(), Runtime> {
	//1st floodfill, close firelocks
	let (planet_turfs, warned_about_space) = {
		let mut cur_queue_idx = 0;
		let mut warned_about_space = false;
		let mut planet_turfs: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
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
					planet_turfs.insert(cur_index);
				}
				if cur_mixture.is_immutable() {
					warned_about_space = true;
					return Ok(());
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
			if warned_about_space {
				break;
			}
		}
		(planet_turfs, warned_about_space)
	};

	if warned_about_space || planet_turfs.is_empty() {
		return Ok(());
	}

	with_turf_gases_read(|arena| {
		let mut progression_order = planet_turfs
			.iter()
			.filter_map(|item| arena.get(*item).map_or_else(|| None, |_| Some(*item)))
			.collect::<IndexSet<_, FxBuildHasher>>();

		let mut cur_queue_idx = 0;
		//2nd floodfill
		while cur_queue_idx < progression_order.len() {
			let cur_index = progression_order[cur_queue_idx];
			cur_queue_idx += 1;

			if cur_queue_idx > equalize_hard_turf_limit {
				continue;
			}

			for adj_index in arena.adjacent_node_ids(cur_index) {
				if arena.get(adj_index).map_or(false, |terf| terf.enabled()) {
					progression_order.insert(adj_index);
				}
			}
		}

		let average_moles = with_planetary_atmos(|map| -> Option<f32> {
			Some(
				map.get(&arena.get(planet_turfs[0])?.planetary_atmos?)?
					.total_moles(),
			)
		});

		if average_moles.is_none() {
			return;
		}
		let average_moles = average_moles.unwrap();

		let (turf, graph) = process_zone(progression_order, average_moles, arena, None);

		let final_pressures = finalize_eq_zone(turf, arena, graph);

		if let Some(final_pressures) = final_pressures {
			let sender = byond_callback_sender();
			send_pressure_differences(final_pressures, &sender);
		}
	});
	Ok(())
}

fn process_zone(
	turfs: IndexSet<NodeIndex<usize>, FxBuildHasher>,
	average_moles: f32,
	arena: &TurfGases,
	turfs_processed: Option<&AtomicUsize>,
) -> (
	IndexSet<NodeIndex<usize>, FxBuildHasher>,
	DiGraphMap<NodeIndex<usize>, f32>,
) {
	let mut graph: DiGraphMap<NodeIndex<usize>, f32> = Default::default();

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
		.partition(|i| info.get(i).unwrap().mole_delta > 0.0);

	let log_n = ((turfs.len() as f32).log2().floor()) as usize;
	if giver_turfs.len() > log_n && taker_turfs.len() > log_n {
		for &cur_index in &turfs {
			monstermos_fast_process(cur_index, arena, &mut info, &mut graph);
		}

		giver_turfs.clear();
		taker_turfs.clear();

		giver_turfs.extend(
			turfs
				.iter()
				.filter(|cur_index| info.get(cur_index).unwrap().mole_delta > 0.0),
		);

		taker_turfs.extend(
			turfs
				.iter()
				.filter(|cur_index| info.get(cur_index).unwrap().mole_delta <= 0.0),
		);
	}

	// alright this is the part that can become O(n^2).
	if giver_turfs.len() < taker_turfs.len() {
		// as an optimization, we choose one of two methods based on which list is smaller.
		give_to_takers(&giver_turfs, arena, &mut info, &mut graph);
	} else {
		take_from_givers(&taker_turfs, arena, &mut info, &mut graph);
	}

	if let Some(ctr) = turfs_processed {
		ctr.fetch_add(turfs.len(), Ordering::Relaxed);
	}

	(turfs, graph)
}

fn finalize_eq_zone(
	turf: IndexSet<NodeIndex<usize>, FxBuildHasher>,
	arena: &TurfGases,
	mut graph: DiGraphMap<NodeIndex<usize>, f32>,
) -> Option<Vec<(f32, u32, u32)>> {
	let mut pressures: Vec<(f32, u32, u32)> = Vec::new();
	turf.iter().for_each(|&cur_index| {
		finalize_eq(cur_index, arena, &mut graph, &mut pressures);
	});
	(!pressures.is_empty()).then_some(pressures)
}

fn send_pressure_differences(
	pressures: Vec<(f32, u32, u32)>,
	sender: &auxcallback::CallbackSender,
) {
	for (amt, cur_turf, adj_turf) in pressures {
		drop(sender.try_send(Box::new(move || {
			let real_amount = Value::from(amt);
			let turf = unsafe { Value::turf_by_id_unchecked(cur_turf) };
			let other_turf = unsafe { Value::turf_by_id_unchecked(adj_turf) };
			if let Err(e) = turf.call("consider_pressure_difference", &[&other_turf, &real_amount])
			{
				Proc::find(byond_string!("/proc/stack_trace"))
					.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
					.call(&[&Value::from_string(e.message.as_str())?])?;
			}
			Ok(())
		})));
	}
}

pub fn equalize(
	equalize_hard_turf_limit: usize,
	high_pressure_turfs: &std::collections::BTreeSet<NodeIndex<usize>>,
	planet_enabled: bool,
) -> usize {
	let turfs_processed: AtomicUsize = AtomicUsize::new(0);
	let mut found_turfs: HashSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
	with_turf_gases_read(|arena| {
		let zoned_turfs = high_pressure_turfs
			.iter()
			.filter_map(|&cur_index| {
				//is this turf already visited?
				if found_turfs.contains(&cur_index) {
					return None;
				};

				//does this turf exists/enabled/have adjacencies?
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

				flood_fill_zones(
					cur_index,
					equalize_hard_turf_limit,
					&mut found_turfs,
					arena,
					planet_enabled,
				)
			})
			.collect::<Vec<_>>();

		if check_turfs_dirty() {
			return;
		}

		let turfs = zoned_turfs
			.into_par_iter()
			.map(|(turfs, total_moles)| {
				let len = turfs.len();
				let (turfs, graph) = process_zone(
					turfs,
					(total_moles / len as f32) as f32,
					arena,
					Some(&turfs_processed),
				);
				(turfs, graph)
			})
			.collect::<Vec<_>>();

		if check_turfs_dirty() {
			return;
		}

		let final_pressures = turfs
			.into_par_iter()
			.filter_map(|(turf, graph)| finalize_eq_zone(turf, arena, graph))
			.collect::<Vec<_>>();

		let sender = byond_callback_sender();

		final_pressures
			.into_iter()
			.for_each(|final_pressures| send_pressure_differences(final_pressures, &sender));
	});
	turfs_processed.load(Ordering::Relaxed)
}
