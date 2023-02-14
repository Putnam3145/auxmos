use super::*;

use coarsetime::{Duration, Instant};

use std::collections::{BTreeSet, HashSet, VecDeque};

lazy_static::lazy_static! {
	static ref GROUPS_CHANNEL: (
		flume::Sender<BTreeSet<NodeIndex>>,
		flume::Receiver<BTreeSet<NodeIndex>>
	) = flume::bounded(1);
}

#[shutdown]
fn flush_groups_channel() {
	with_groups_info_receiver(|recv| _ = recv.try_recv())
}

fn with_groups_info_receiver<T>(f: impl Fn(&flume::Receiver<BTreeSet<NodeIndex>>) -> T) -> T {
	f(&GROUPS_CHANNEL.1)
}

pub fn equalize_groups_sender() -> flume::Sender<BTreeSet<NodeIndex>> {
	GROUPS_CHANNEL.0.clone()
}

#[hook("/datum/controller/subsystem/air/proc/process_excited_groups_auxtools")]
fn _groups_hook(remaining: Value) {
	let group_pressure_goal = src
		.get_number(byond_string!("excited_group_pressure_goal"))
		.unwrap_or(0.5);
	let remaining_time = Duration::from_millis(remaining.as_number().unwrap_or(50.0) as u64);
	let start_time = Instant::now();
	let (num_eq, is_cancelled) = with_groups_info_receiver(|recv| {
		if let Ok(high_pressure_turfs) = recv.try_recv() {
			excited_group_processing(
				group_pressure_goal,
				high_pressure_turfs,
				(&start_time, remaining_time),
			)
		} else {
			(0, false)
		}
	});

	let bench = start_time.elapsed().as_millis();
	let prev_cost = src.get_number(byond_string!("cost_groups")).map_err(|_| {
		runtime!(
			"Attempt to interpret non-number value as number {} {}:{}",
			std::file!(),
			std::line!(),
			std::column!()
		)
	})?;
	src.set(
		byond_string!("cost_groups"),
		Value::from(0.8 * prev_cost + 0.2 * (bench as f32)),
	)?;
	src.set(
		byond_string!("num_group_turfs_processed"),
		Value::from(num_eq as f32),
	)?;
	Ok(Value::from(is_cancelled))
}

// Finds small differences in turf pressures and equalizes them.
fn excited_group_processing(
	pressure_goal: f32,
	low_pressure_turfs: BTreeSet<NodeIndex>,
	(start_time, remaining_time): (&Instant, Duration),
) -> (usize, bool) {
	let mut found_turfs: HashSet<NodeIndex, FxBuildHasher> = Default::default();
	let mut is_cancelled = false;
	with_turf_gases_read(|arena| {
		for initial_turf in low_pressure_turfs {
			if found_turfs.contains(&initial_turf) {
				continue;
			}

			if start_time.elapsed() >= remaining_time || check_turfs_dirty() {
				is_cancelled = true;
				return;
			}

			let Some(initial_mix_ref) = arena.get(initial_turf) else { continue; };
			if !initial_mix_ref.enabled() {
				continue;
			}

			let mut border_turfs: VecDeque<NodeIndex> = VecDeque::with_capacity(40);
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
							arena
								.adjacent_node_ids(idx)
								.filter(|&loc| found_turfs.insert(loc))
								.filter(|&loc| arena.get(loc).filter(|b| b.enabled()).is_some())
								.for_each(|loc| border_turfs.push_back(loc));
						}
					} else {
						break;
					}
				}
				fully_mixed.multiply(1.0 / turfs.len() as f32);
				if !fully_mixed.is_corrupt() {
					turfs
						.par_iter()
						.filter_map(|turf| all_mixtures.get(turf.mix))
						.for_each(|mix_lock| mix_lock.write().copy_from_mutable(&fully_mixed));
				}
			});
		}
	});
	(found_turfs.len(), is_cancelled)
}
