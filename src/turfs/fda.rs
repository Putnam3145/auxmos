use super::*;

use crate::GasMixtures;

/*
A finite difference analysis diffusion engine.

Replaces LINDA, since the overhead of keeping active turfs isn't worth the performance gain.

Yeah, it's seriously fast enough to just operate on every turf now, go figure.
*/

// turf id, bitflags (1 = should update visuals, 2 = should react), pressure diffs
type TurfProcessInfo = Option<(u32, u32, [(u32, f32); 6])>;

lazy_static! {
	static ref TURF_PROCESS_CHANNEL: (
		flume::Sender<TurfProcessInfo>,
		flume::Receiver<TurfProcessInfo>
	) = flume::unbounded();
}

const TURFS_NOT_PROCESSING: u8 = 0;
const TURFS_PROCESSING: u8 = 1;
const TURFS_DONE_PROCESSING: u8 = 2;

static TURF_PROCESS_STEP: AtomicU8 = AtomicU8::new(TURFS_NOT_PROCESSING);

#[hook("/datum/controller/subsystem/air/proc/process_turfs_extools")]
fn _process_turf_hook() {
	/*
		This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
		which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
		This is a much simpler FDM system, basically like LINDA but without its most important feature (sleeping turfs).
		It can run in parallel, but doesn't yet. We'll see if it's required for performance reasons.
	*/
	// First we copy the gas list immutably, so we can be sure this is consistent.
	if TURF_PROCESS_STEP.compare_and_swap(TURFS_NOT_PROCESSING, TURFS_PROCESSING, Ordering::Relaxed)
		== TURFS_NOT_PROCESSING
	{
		rayon::spawn(move || {
			let max_x = TurfGrid::max_x();
			let max_y = TurfGrid::max_y();
			let mut turfs_to_save: Vec<(usize, TurfMixture, GasMixture, [(u32, f32); 6])> =
				TURF_GASES
					.iter()
					.par_bridge()
					.filter_map(|e| {
						let (i, m) = (*e.key(), *e.value());
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
											if gas.compare(&adj_gas, MINIMUM_MOLES_DELTA_TO_MOVE) {
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
														mix.return_pressure()
															* GAS_DIFFUSION_CONSTANT,
													);
												}
											}
										}
										j += 1;
									}
								});
								if let Some(planet_atmos) = m.planetary_atmos {
									end_gas
										.merge(PLANETARY_ATMOS.get(planet_atmos).unwrap().value());
								}
								end_gas.multiply(GAS_DIFFUSION_CONSTANT);
								Some((i, m, end_gas, pressure_diffs))
							} else {
								None
							}
						} else {
							None
						}
					})
					.collect();
			let sender = TURF_PROCESS_CHANNEL.0.clone();
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
							gas.multiply(1.0 - (GAS_DIFFUSION_CONSTANT * adj_amount as f32));
							let cur_pressure = gas.return_pressure();
							for pressure_diff in pressure_diffs.iter_mut() {
								if pressure_diff.1 != 0.0 {
									pressure_diff.1 += cur_pressure;
								}
							}
							gas.merge(&end_gas);
							if gas.is_visible() {
								flags |= 1;
							}
							if gas.can_react() {
								flags |= 2;
							}
							if flags > 0 {
								sender
									.send(Some((*i as u32, flags, *pressure_diffs)))
									.unwrap();
							}
						}
					});
				});
			TURF_PROCESS_STEP.store(TURFS_DONE_PROCESSING, Ordering::Relaxed);
		});
	}
	let time_limit = Duration::from_millis(
		args.get(0)
			.ok_or_else(|| runtime!("Wrong number of arguments to process_turfs_extools"))?
			.as_number()? as u64,
	);
	let wait_time = std::time::Duration::new(time_limit.as_secs(), time_limit.subsec_nanos()) / 10;
	let start_time = Instant::now();
	let receiver = TURF_PROCESS_CHANNEL.1.clone();
	let mut done = false;
	while !done && start_time.elapsed() < time_limit {
		if let Ok(res) = receiver.recv_timeout(wait_time) {
			if let Some((i, flags, pressure_diffs)) = res {
				let turf = TurfGrid::turf_by_id(i);
				if flags & 2 == 2 {
					if let Err(e) = turf.get("air").unwrap().call("react", &[turf.clone()]) {
						src.call("stack_trace", &[&Value::from_string(e.message.as_str())])?;
					}
				}
				if flags & 1 == 1 {
					if let Err(e) = turf.call("update_visuals", &[Value::null()]) {
						src.call("stack_trace", &[&Value::from_string(e.message.as_str())])?;
					}
				}
				for &(j, diff) in pressure_diffs.iter() {
					if diff.abs() > 0.0 {
						let other_turf = TurfGrid::turf_by_id(j);
						if diff < 0.0 {
							if let Err(e) = other_turf.call(
								"consider_pressure_difference",
								&[turf.clone(), Value::from(diff)],
							) {
								src.call(
									"stack_trace",
									&[&Value::from_string(e.message.as_str())],
								)?;
							}
						} else {
							if let Err(e) = turf.call(
								"consider_pressure_difference",
								&[other_turf.clone(), Value::from(diff)],
							) {
								src.call(
									"stack_trace",
									&[&Value::from_string(e.message.as_str())],
								)?;
							}
						}
					}
				}
			}
		} else {
			done = TURF_PROCESS_STEP.compare_and_swap(
				TURFS_DONE_PROCESSING,
				TURFS_NOT_PROCESSING,
				Ordering::Relaxed,
			) == TURFS_DONE_PROCESSING;
		}
	}
	if done {
		Ok(Value::from(0.0))
	} else {
		Ok(Value::from(1.0))
	}
}
