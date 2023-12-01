use super::*;

use coarsetime::{Duration, Instant};

use std::collections::{BTreeSet, HashSet, VecDeque};

use parking_lot::{const_mutex, Mutex};

static GROUPS_CHANNEL: Mutex<Option<BTreeSet<TurfID>>> = const_mutex(None);

pub fn flush_groups_channel() {
	*GROUPS_CHANNEL.lock() = None;
}

fn with_groups<T>(f: impl Fn(Option<BTreeSet<TurfID>>) -> T) -> T {
	f(GROUPS_CHANNEL.lock().take())
}

pub fn send_to_groups(sent: BTreeSet<TurfID>) {
	GROUPS_CHANNEL.try_lock().map(|mut opt| opt.replace(sent));
}

#[byondapi_binds::bind("/datum/controller/subsystem/air/proc/process_excited_groups_auxtools")]
fn groups_hook(mut src: ByondValue, remaining: ByondValue) {
	let group_pressure_goal = src
		.read_number_id(byond_string!("excited_group_pressure_goal"))
		.unwrap_or(0.5);
	let remaining_time = Duration::from_millis(remaining.get_number().unwrap_or(50.0) as u64);
	let start_time = Instant::now();
	let (num_eq, is_cancelled) = with_groups(|thing| {
		if let Some(high_pressure_turfs) = thing {
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
	let prev_cost = src
		.read_number_id(byond_string!("cost_groups"))
		.map_err(|_| {
			eyre::eyre!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})?;
	src.write_var(
		"cost_groups",
		&(0.8 * prev_cost + 0.2 * (bench as f32)).into(),
	)?;
	src.write_var("num_group_turfs_processed", &(num_eq as f32).into())?;
	Ok(is_cancelled.into())
}

// Finds small differences in turf pressures and equalizes them.
fn excited_group_processing(
	pressure_goal: f32,
	low_pressure_turfs: BTreeSet<TurfID>,
	(start_time, remaining_time): (&Instant, Duration),
) -> (usize, bool) {
	let mut found_turfs: HashSet<TurfID, FxBuildHasher> = Default::default();
	let mut is_cancelled = false;
	for initial_turf in low_pressure_turfs {
		if found_turfs.contains(&initial_turf) {
			continue;
		}

		if start_time.elapsed() >= remaining_time {
			is_cancelled = true;
			break;
		}

		with_turf_gases_read(|arena| {
			let Some(initial_mix_ref) = arena.get_from_id(initial_turf) else {
				return;
			};
			if !initial_mix_ref.enabled() {
				return;
			}

			let mut border_turfs: VecDeque<TurfID> = VecDeque::with_capacity(40);
			let mut turfs: Vec<&TurfMixture> = Vec::with_capacity(200);
			let mut min_pressure = initial_mix_ref.return_pressure();
			let mut max_pressure = min_pressure;
			let mut fully_mixed = Mixture::new();

			border_turfs.push_back(initial_turf);
			found_turfs.insert(initial_turf);
			GasArena::with_all_mixtures(|all_mixtures| {
				loop {
					if turfs.len() >= 2500 {
						break;
					}
					if let Some(idx) = border_turfs.pop_front() {
						let Some(tmix) = arena.get_from_id(idx) else {
							break;
						};
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
								.adjacent_turf_ids(arena.get_id(idx).unwrap())
								.filter(|&loc| found_turfs.insert(loc))
								.filter(|&loc| {
									arena.get_from_id(loc).filter(|b| b.enabled()).is_some()
								})
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
		});
	}
	(found_turfs.len(), is_cancelled)
}
