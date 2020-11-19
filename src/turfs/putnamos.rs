use super::*;

use std::collections::VecDeque;

use std::collections::BTreeSet;

use std::sync::atomic::{AtomicU8, Ordering};

const PUTNAMOS_NONE: u8 = 0;
const PUTNAMOS_RUNNING: u8 = 1;
const PUTNAMOS_DONE: u8 = 2;

static PUTNAMOS_STEP: AtomicU8 = AtomicU8::new(PUTNAMOS_NONE);

#[hook("/datum/controller/subsystem/air/proc/process_turf_equalize_extools")]
fn _hook_equalize() {
	/*
		When FDA detects a high pressure delta, it will put the turf with that delta
		into the high pressure channel, which can then be consumed by other atmos
		subsystems. Monstermos is monster860's; this is mine.

		First, much like Monstermos, we gather all the giver and taker turfs.
		However, unlike monstermos, our goal is to merely "speed up" the diffusion--
		rather than having an arbitrary turf limit, we keep spreading the "taker" turfs
		until the largest pressure gradient is below an arbitrary threshold.
	*/
	let cb = Callback::new(args.get(1).unwrap())?;
	let max_x = ctx.get_world().get_number("maxx")? as i32;
	let max_y = ctx.get_world().get_number("maxy")? as i32;
	let turf_receiver = HIGH_PRESSURE_TURFS.1.clone();
	let mut found_turfs: BTreeSet<usize> = BTreeSet::new();
	if PUTNAMOS_STEP.load(Ordering::SeqCst) == PUTNAMOS_NONE {
		rayon::spawn(move || {
			PUTNAMOS_STEP.store(PUTNAMOS_RUNNING, Ordering::SeqCst);
			for initial_turf in turf_receiver.try_iter() {
				if found_turfs.contains(&initial_turf) {
					continue;
				}
				// floodfills is the same
				// array here is north of tile, right of tile, up of tile; staggered grid setup.
				let mut turfs: Vec<(usize, TurfMixture, [f32; 2])> = Vec::with_capacity(60);
				let mut border_turfs: VecDeque<(usize, TurfMixture)> = VecDeque::with_capacity(20);
				let mut big_gas = GasMixture::new();
				border_turfs.push_back((
					initial_turf,
					*TURF_GASES.get(&initial_turf).unwrap().value(),
				));
				loop {
					if border_turfs.is_empty() {
						break;
					}
					let mut pressure_del_found = false;
					let mut our_border_turfs: VecDeque<(usize, TurfMixture)> =
						VecDeque::with_capacity(border_turfs.len() + 4);
					for (i, turf) in border_turfs.drain(..) {
						if found_turfs.contains(&i) {
							continue;
						}
						found_turfs.insert(i);
						GasMixtures::with_all_mixtures(|all_mixtures| {
							let to_merge = all_mixtures
								.get(turf.mix)
								.expect(&format!("Gas mixture not found for turf: {}", turf.mix))
								.read()
								.unwrap();
							big_gas.merge(&to_merge);
							big_gas.volume += to_merge.volume;
						});
						let big_gas_pressure = big_gas.return_pressure();
						let mut diffs = [0.0, 0.0];
						for &(j, loc) in adjacent_tile_ids(turf.adjacency, i, max_x, max_y).iter() {
							let adj_turf = *TURF_GASES.get(&loc).unwrap().value();
							let pressure_delta =
								big_gas_pressure - adj_turf.return_pressure();
							our_border_turfs.push_back((loc, adj_turf));
							match 1 << j {
								NORTH => diffs[0] = pressure_delta,
								EAST => diffs[1] = pressure_delta,
								_ => (),
							}
							if pressure_delta.abs() > 1.0 {
								pressure_del_found = true
							}
						}
						turfs.push((i, turf, diffs));
					}
					if !pressure_del_found {
						break;
					}
					border_turfs.append(&mut our_border_turfs);
				}
				let should_update_visuals = big_gas.is_visible();
				big_gas.multiply((turfs.len() as f32).recip());
				big_gas.volume = 2500.0;
				turfs.par_iter().for_each(|&(i, turf, diffs)| {
					GasMixtures::with_all_mixtures(|all_mixtures| {
						let mut turf_mix = all_mixtures
							.get(turf.mix)
							.expect(&format!("Gas mixture not found for turf: {}", turf.mix))
							.write()
							.unwrap();
						if !turf_mix.is_immutable() {
							turf_mix.copy_from_mutable(&big_gas);
						}
					});
					if should_update_visuals || diffs.iter().any(|p| p.abs() > f32::EPSILON) {
						cb.invoke(move || {
							let turf = unsafe { Value::turf_by_id_unchecked(i as u32) };
							let true_pressure_diffs = List::new();
							for &diff in diffs.iter() {
								true_pressure_diffs.append(&Value::from(diff));
							}
							vec![turf, Value::from(true_pressure_diffs)]
						});
					}
				});
			}
			PUTNAMOS_STEP.store(PUTNAMOS_DONE, Ordering::SeqCst);
		});
	}
	Ok(Value::from(
		PUTNAMOS_STEP.compare_and_swap(PUTNAMOS_DONE, PUTNAMOS_NONE, Ordering::SeqCst)
			!= PUTNAMOS_DONE,
	))
}
