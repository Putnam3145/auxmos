use super::*;
use byondapi::prelude::*;
//use indexmap::IndexSet;
use crate::GasArena;
use auxcallback::byond_callback_sender;
use coarsetime::Instant;
use eyre::Result;
use parking_lot::Once;

static INIT_HEAT: Once = Once::new();

static TURF_HEAT: RwLock<Option<TurfHeat>> = const_rwlock(None);

lazy_static::lazy_static! {
	static ref HEAT_CHANNEL: (flume::Sender<SSheatInfo>, flume::Receiver<SSheatInfo>) =
		flume::bounded(1);
}

#[init(partial)]
fn initialize_heat_statics() -> Result<(), String> {
	*TURF_HEAT.write() = Some(TurfHeat {
		graph: StableDiGraph::with_capacity(650_250, 1_300_500),
		map: IndexMap::with_capacity_and_hasher(650_250, FxBuildHasher::default()),
	});
	Ok(())
}

#[shutdown]
fn shutdown_turfs() {
	wait_for_tasks();
	*TURF_HEAT.write() = None;
}

fn with_turf_heat_read<T, F>(f: F) -> T
where
	F: FnOnce(&TurfHeat) -> T,
{
	f(TURF_HEAT.read().as_ref().unwrap())
}

fn with_turf_heat_write<T, F>(f: F) -> T
where
	F: FnOnce(&mut TurfHeat) -> T,
{
	f(TURF_HEAT.write().as_mut().unwrap())
}

#[derive(Copy, Clone)]
struct SSheatInfo {
	time_delta: f64,
}

#[derive(Default)]
struct ThermalInfo {
	pub id: TurfID,

	pub thermal_conductivity: f32,
	pub heat_capacity: f32,
	pub adjacent_to_space: bool,

	pub temperature: RwLock<f32>,
}

fn with_heat_processing_callback_receiver<T>(f: impl Fn(&flume::Receiver<SSheatInfo>) -> T) -> T {
	f(&HEAT_CHANNEL.1)
}

fn heat_processing_callbacks_sender() -> flume::Sender<SSheatInfo> {
	HEAT_CHANNEL.0.clone()
}
type HeatGraphMap = IndexMap<TurfID, NodeIndex<usize>, FxBuildHasher>;

//turf temperature infos goes here
struct TurfHeat {
	graph: StableDiGraph<ThermalInfo, (), usize>,
	map: HeatGraphMap,
}

impl TurfHeat {
	pub fn insert_turf(&mut self, info: ThermalInfo) {
		if let Some(&node_id) = self.map.get(&info.id) {
			let thin = self.graph.node_weight_mut(node_id).unwrap();
			thin.thermal_conductivity = info.thermal_conductivity;
			thin.heat_capacity = info.heat_capacity;
			thin.adjacent_to_space = info.adjacent_to_space;
		} else {
			self.map.insert(info.id, self.graph.add_node(info));
		}
	}

	pub fn remove_turf(&mut self, id: TurfID) {
		if let Some(index) = self.map.remove(&id) {
			self.graph.remove_node(index);
		}
	}

	pub fn get(&self, idx: NodeIndex<usize>) -> Option<&ThermalInfo> {
		self.graph.node_weight(idx)
	}

	pub fn get_id(&self, idx: &TurfID) -> Option<&NodeIndex<usize>> {
		self.map.get(idx)
	}

	pub fn adjacent_node_ids<'a>(
		&'a self,
		index: NodeIndex<usize>,
	) -> impl Iterator<Item = NodeIndex<usize>> + '_ {
		self.graph.neighbors(index)
	}

	pub fn adjacent_heats(
		&self,
		index: NodeIndex<usize>,
	) -> impl Iterator<Item = &ThermalInfo> + '_ {
		self.graph
			.neighbors(index)
			.filter_map(|neighbor| self.graph.node_weight(neighbor))
	}

	pub fn update_adjacencies(
		&mut self,
		idx: TurfID,
		blocked_dirs: Directions,
		max_x: i32,
		max_y: i32,
	) {
		if let Some(&this_node) = self.get_id(&idx) {
			self.remove_adjacencies(this_node);
			for (_, adj_idx) in adjacent_tile_ids(
				Directions::ALL_CARDINALS_MULTIZ - blocked_dirs,
				idx,
				max_x,
				max_y,
			) {
				if let Some(&adjacent_node) = self.get_id(&adj_idx) {
					//this fucking happens, I don't even know anymore
					if adjacent_node != this_node {
						self.graph.add_edge(this_node, adjacent_node, ());
					}
				}
			}
		}
	}

	pub fn remove_adjacencies(&mut self, index: NodeIndex<usize>) {
		let edges = self
			.graph
			.edges(index)
			.map(|edgeref| edgeref.id())
			.collect::<Vec<_>>();
		edges.into_iter().for_each(|edgeindex| {
			self.graph.remove_edge(edgeindex);
		});
	}
}

pub fn supercond_update_ref(src: ByondValue) -> Result<()> {
	let id = unsafe { src.raw.data.id };
	let therm_cond = src.read_number("thermal_conductivity").unwrap_or(0.0);
	let therm_cap = src.read_number("heat_capacity").unwrap_or(0.0);
	if therm_cond > 0.0 && therm_cap > 0.0 {
		let therm_info = ThermalInfo {
			id,
			adjacent_to_space: src
				.call_id(byond_string!("should_conduct_to_space"), &[])?
				.as_number()?
				> 0.0,
			heat_capacity: therm_cap,
			thermal_conductivity: therm_cond,
			temperature: RwLock::new(src.read_number("initial_temperature").unwrap_or(TCMB)),
		};
		with_turf_heat_write(|arena| arena.insert_turf(therm_info));
	} else {
		with_turf_heat_write(|arena| arena.remove_turf(id));
	}
	Ok(())
}

pub fn supercond_update_adjacencies(id: u32) -> Result<()> {
	let max_x = auxtools::ByondValue::world()
		.read_number("maxx")
		.map_err(|_| {
			eyre::eyre!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})? as i32;
	let max_y = auxtools::ByondValue::world()
		.read_number("maxy")
		.map_err(|_| {
			eyre::eyre!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})? as i32;
	let src_turf = unsafe { ByondValue::turf_by_id_unchecked(id) };
	with_turf_heat_write(|arena| -> Result<()> {
		if let Ok(blocked_dirs) = src_turf.read_number("conductivity_blocked_directions") {
			let actual_dir = Directions::from_bits_truncate(blocked_dirs as u8);
			arena.update_adjacencies(id, actual_dir, max_x, max_y)
		} else if let Some(&idx) = arena.get_id(&id) {
			arena.remove_adjacencies(idx)
		}
		Ok(())
	})?;
	Ok(())
}

#[byondapi_hooks::bind("/turf/proc/return_temperature")]
fn hook_turf_temperature() -> Result<ByondValue> {
	with_turf_heat_read(|arena| -> Result<ByondValue> {
		if let Some(&node_index) = arena.get_id(&unsafe { src.raw.data.id }) {
			let info = arena.get(node_index).unwrap();
			let read = info.temperature.read();
			if read.is_normal() {
				Ok(ByondValue::from(*read))
			} else {
				Ok(ByondValue::from(300))
			}
		} else {
			Ok(ByondValue::from(102))
		}
	})
}

// Expected function call: process_turf_heat()
// Returns: TRUE if thread not done, FALSE otherwise
#[byondapi_hooks::bind("/datum/controller/subsystem/air/proc/process_turf_heat")]
fn process_heat_notify() -> Result<ByondValue> {
	rebuild_turf_graph()?;
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
	let time_delta = (src.read_number("wait").map_err(|_| {
		eyre::eyre!(
			"Attempt to interpret non-number value as number {} {}:{}",
			std::file!(),
			std::line!(),
			std::column!()
		)
	})? / 10.0) as f64;
	_ = sender.try_send(SSheatInfo { time_delta });
	Ok(ByondValue::null())
}

fn get_share_energy(delta: f32, cap_1: f32, cap_2: f32) -> f32 {
	delta * ((cap_1 * cap_2) / (cap_1 + cap_2))
}

//Fires the task into the thread pool, once
#[init(full)]
fn process_heat_start() -> Result<(), String> {
	INIT_HEAT.call_once(|| {
		rayon::spawn(|| loop {
			//this will block until process_turf_heat is called
			let info = with_heat_processing_callback_receiver(|receiver| receiver.recv().unwrap());
			let task_lock = TASKS.read();
			let start_time = Instant::now();
			let sender = byond_callback_sender();
			let _emissivity_constant: f64 = STEFAN_BOLTZMANN_CONSTANT * info.time_delta;
			let _radiation_from_space_tick: f64 = RADIATION_FROM_SPACE * info.time_delta;
			with_turf_heat_read(|arena| {
				with_turf_gases_read(|air_arena| {
					let adjacencies_to_consider = arena
						.map
						.par_iter()
						.filter_map(|(&turf_id, &heat_index)| {
							/*
								If it has no thermal conductivity, low thermal capacity or has no adjacencies,
								then it's not gonna interact, or at least shouldn't.
							*/
							let info = arena.get(heat_index).unwrap();
							let temp = { *info.temperature.read() };
							//can share w/ adjacents?
							if arena.adjacent_heats(heat_index).any(|item| {
								(temp - *item.temperature.read()).abs()
									> MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER
							}) {
								return Some((turf_id, heat_index, true));
							}
							if temp > MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION {
								//can share w/ space/air?
								if info.adjacent_to_space
									|| air_arena
										.get_id(&turf_id)
										.and_then(|&nodeid| {
											air_arena.get(nodeid)?.enabled().then(|| ())
										})
										.is_some()
								{
									Some((turf_id, heat_index, false))
								} else {
									None
								}
							} else if let Some(node) = air_arena.get_id(&turf_id) {
								let cur_mix = air_arena.get(*node).unwrap();
								if !cur_mix.enabled() {
									return None;
								}
								GasArena::with_all_mixtures(|all_mixtures| {
									let air_temp = all_mixtures[cur_mix.mix].try_read();
									if air_temp.is_none() {
										return false;
									}
									let air_temp = air_temp.unwrap().get_temperature();

									if air_temp < MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION {
										return false;
									}
									(temp - air_temp).abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER
								})
								.then(|| (turf_id, heat_index, false))
							} else {
								None
							}
						})
						.filter_map(|(id, node_index, has_adjacents)| {
							let info = arena.get(node_index).unwrap();
							let mut temp_write = info.temperature.try_write()?;

							/*
							//share w/ space
							if info.adjacent_to_space && *temp_write > T0C {
								/*
									Straight up the standard blackbody radiation
									equation. All these are f64s because
									f32::MAX^4 < f64::MAX, and t.temperature
									is ordinarily an f32, meaning that
									this will never go into infinities.
								*/
								let blackbody_radiation: f64 = (emissivity_constant
									* STEFAN_BOLTZMANN_CONSTANT
									* (f64::from(*temp_write).powi(4)))
									- radiation_from_space_tick;
								*temp_write -= blackbody_radiation as f32 / info.heat_capacity;
							}
							*/
							//share w/ space
							if info.adjacent_to_space && *temp_write > T20C {
								let delta = *temp_write - TCMB;
								let energy = get_share_energy(
									info.thermal_conductivity * delta,
									HEAT_CAPACITY_VACUUM,
									info.heat_capacity,
								);
								*temp_write -= energy / info.heat_capacity;
							}

							//share w/ air
							if let Some(&id) = air_arena.get_id(&id) {
								let tmix = air_arena.get(id).unwrap();
								if tmix.enabled() {
									GasArena::with_all_mixtures(|all_mixtures| {
										if let Some(entry) = all_mixtures.get(tmix.mix) {
											if let Some(mut gas) = entry.try_write() {
												*temp_write = gas.temperature_share_non_gas(
													/*
														This value should be lower than the
														turf-to-turf conductivity for balance reasons
														as well as realism, otherwise fires will
														just sort of solve theirselves over time.
													*/
													info.thermal_conductivity
														* OPEN_HEAT_TRANSFER_COEFFICIENT,
													*temp_write,
													info.heat_capacity,
												);
											}
										}
									})
								}
							}

							if !temp_write.is_normal() {
								*temp_write = TCMB;
							}

							if *temp_write > MINIMUM_TEMPERATURE_START_SUPERCONDUCTION
								&& *temp_write > info.heat_capacity
							{
								// not what heat capacity means but whatever
								drop(sender.try_send(Box::new(move || {
									let turf = unsafe { ByondValue::turf_by_id_unchecked(id) };
									turf.set("to_be_destroyed", 1.0)?;
									Ok(())
								})));
							}
							has_adjacents.then(|| node_index)
						})
						.collect::<Vec<_>>();

					_ = adjacencies_to_consider
						.par_iter()
						.try_for_each(|&cur_index| {
							let info = arena.get(cur_index).unwrap();
							if let Some(mut temp_write) = info.temperature.try_write() {
								//share w/ adjacents that are strictly in zone
								for other in arena
									.adjacent_node_ids(cur_index)
									.filter_map(|idx| arena.get(idx))
								{
									/*
										The horrible line below is essentially
										sharing between solids--making it the minimum of both
										conductivities makes this consistent, funnily enough.
									*/
									if let Some(mut other_write) = other.temperature.try_write() {
										let shareds =
											info.thermal_conductivity
												.min(other.thermal_conductivity) * get_share_energy(
												*other_write - *temp_write,
												info.heat_capacity,
												other.heat_capacity,
											);
										*temp_write += shareds / info.heat_capacity;
										*other_write -= shareds / other.heat_capacity;
									}
								}
							}
							Ok(())
						});
				});
			});
			let bench = start_time.elapsed().as_millis();
			drop(sender.try_send(Box::new(move || {
				let ssair = auxtools::ByondValue::globals().get("SSair")?;
				let prev_cost = ssair.read_number("cost_superconductivity").map_err(|_| {
					eyre::eyre!(
						"Attempt to interpret non-number value as number {} {}:{}",
						std::file!(),
						std::line!(),
						std::column!()
					)
				})?;
				ssair.set(
					"cost_superconductivity",
					ByondValue::from(0.8 * prev_cost + 0.2 * (bench as f32)),
				)?;
				Ok(())
			})));
			drop(task_lock);
		});
	});
	Ok(())
}
/*

fn flood_fill_temps(
	input: Vec<NodeIndex<usize>>,
	arena: &TurfHeat,
) -> Vec<IndexSet<NodeIndex<usize>, FxBuildHasher>> {
	let mut found_turfs: HashSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
	let mut return_val: Vec<IndexSet<NodeIndex<usize>, FxBuildHasher>> = Default::default();
	for temp_id in input {
		let mut turfs: IndexSet<NodeIndex<usize>, FxBuildHasher> = Default::default();
		let mut border_turfs: std::collections::VecDeque<NodeIndex<usize>> = Default::default();
		border_turfs.push_back(temp_id);
		found_turfs.insert(temp_id);
		while let Some(cur_index) = border_turfs.pop_front() {
			for adj_index in arena.adjacent_node_ids(cur_index) {
				if found_turfs.insert(adj_index) {
					border_turfs.push_back(adj_index)
				}
			}
			turfs.insert(cur_index);
		}
		return_val.push(turfs)
	}
	return_val
}
*/
