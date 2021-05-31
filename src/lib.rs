#[macro_use]
extern crate lazy_static;

pub mod gas;

pub mod turfs;

use auxtools::*;

use gas::*;

use gas::reaction::react_by_id;

use gas::constants::*;

#[hook("/proc/process_atmos_callbacks")]
fn _atmos_callback_handle() {
	auxcallback::callback_processing_hook(args)
}

#[hook("/datum/gas_mixture/proc/__gasmixture_register")]
fn _register_gasmixture_hook() {
	gas::GasMixtures::register_gasmix(src)
}

#[hook("/datum/gas_mixture/proc/__gasmixture_unregister")]
fn _unregister_gasmixture_hook() {
	gas::GasMixtures::unregister_gasmix(src)
}

#[hook("/datum/gas_mixture/proc/heat_capacity")]
fn _heat_cap_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.heat_capacity())))
}

#[hook("/datum/gas_mixture/proc/set_min_heat_capacity")]
fn _min_heat_cap_hook() {
	let cap = args
		.get(0)
		.ok_or_else(|| runtime!("attempted to set min heat capacity with no argument"))?
		.as_number()
		.unwrap_or_default();
	with_mix_mut(src, move |mix| {
		mix.set_min_heat_capacity(cap);
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/total_moles")]
fn _total_moles_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.total_moles())))
}

#[hook("/datum/gas_mixture/proc/return_pressure")]
fn _return_pressure_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.return_pressure())))
}

#[hook("/datum/gas_mixture/proc/return_temperature")]
fn _return_temperature_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.get_temperature())))
}

#[hook("/datum/gas_mixture/proc/return_volume")]
fn _return_volume_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.volume)))
}

#[hook("/datum/gas_mixture/proc/thermal_energy")]
fn _thermal_energy_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.thermal_energy())))
}

#[hook("/datum/gas_mixture/proc/merge")]
fn _merge_hook(mix: Value) {
	with_mixes_mut(src, &mix, move |src_mix, giver_mix| {
		src_mix.merge(giver_mix);
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/__remove_ratio")]
fn _remove_ratio_hook(mix: Value, ratio_val: Value) {
	let ratio = ratio_val.as_number().unwrap_or_default();
	with_mixes_mut(src, &mix, move |src_mix, into_mix| {
		&src_mix.remove_ratio(ratio, into_mix);
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/__remove")]
fn _remove_hook(mix: Value, amt_val: Value) {
	let amt = amt_val.as_number().unwrap_or_default();
	with_mixes_mut(src, &mix, move |src_mix, into_mix| {
		&src_mix.remove(amt, into_mix);
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/copy_from")]
fn _copy_from_hook(mix: Value) {
	with_mixes_mut(src, &mix, move |src_mix, giver_mix| {
		src_mix.copy_from_mutable(giver_mix);
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/temperature_share")]
fn _temperature_share_hook() {
	let arg_num = args.len();
	match arg_num {
		2 => {
			let coefficient = args[1].as_number().unwrap_or_default();
			with_mixes_mut(src, &args[0], move |src_mix, share_mix| {
				Ok(Value::from(
					src_mix.temperature_share(share_mix, coefficient),
				))
			})
		}
		4 => {
			let other_temp = args[0].as_number().unwrap_or_default();
			let other_cap = args[1].as_number().unwrap_or_default();
			let coefficient = args[2].as_number().unwrap_or_default();
			with_mix_mut(src, move |mix| {
				Ok(Value::from(mix.temperature_share_non_gas(
					other_temp,
					other_cap,
					coefficient,
				)))
				.map(|ret| {
					if ret == Value::null() {
						Value::from(other_temp)
					} else {
						ret
					}
				})
			})
		}
		_ => Err(runtime!("Invalid args for temperature_share")),
	}
}

#[hook("/datum/gas_mixture/proc/get_gases")]
fn _get_gases_hook() {
	with_mix(src, |mix| {
		let gases_list: List = List::new();
		for gas in mix.get_gases() {
			gases_list.append(&gas_id_to_type(gas)?);
		}
		Ok(Value::from(gases_list))
	})
}

#[hook("/datum/gas_mixture/proc/set_temperature")]
fn _set_temperature_hook(v: Value) {
	let temp = v.as_number()?;
	if !temp.is_finite() {
		Err(runtime!(
			"Attempted to set a temperature to a number that is NaN or infinite."
		))
	} else {
		with_mix_mut(src, move |mix| {
			mix.set_temperature(temp.max(2.7));
			Ok(Value::null())
		})
	}
}

#[hook("/datum/gas_mixture/proc/partial_heat_capacity")]
fn _partial_heat_capacity() {
	if args.is_empty() {
		Err(runtime!("Incorrect arg len for partial_heat_capacity (0)."))
	} else {
		with_mix(src, |mix| {
			Ok(Value::from(
				mix.partial_heat_capacity(gas_id_from_type(&args[0])?),
			))
		})
	}
}

#[hook("/datum/gas_mixture/proc/set_volume")]
fn _set_volume_hook(vol_val: Value) {
	let vol = vol_val.as_number()?;
	with_mix_mut(src, move |mix| {
		mix.volume = vol;
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/get_moles")]
fn _get_moles_hook() {
	if args.is_empty() {
		Err(runtime!("Incorrect arg len for get_moles (0)."))
	} else {
		with_mix(src, |mix| {
			Ok(Value::from(mix.get_moles(gas_id_from_type(&args[0])?)))
		})
	}
}

#[hook("/datum/gas_mixture/proc/set_moles")]
fn _set_moles_hook(gas_type: Value, vf: Value) {
	let amt = vf.as_number().unwrap_or_default();
	if !amt.is_finite() {
		return Err(runtime!("Attempted to set moles to NaN or infinity."));
	}
	if amt < 0.0 {
		return Err(runtime!("Attempted to set moles to a negative number."));
	}
	let gas_id = gas_id_from_type(&gas_type)?;
	with_mix_mut(src, move |mix| {
		mix.set_moles(gas_id, amt);
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/adjust_moles")]
fn _adjust_moles_hook(gas_type: Value, vf: Value) {
	let amt = vf.as_number().unwrap_or_default();
	if !amt.is_finite() {
		return Err(runtime!("Attempted to adjust moles by NaN or infinity."));
	}
	let gas_id = gas_id_from_type(&gas_type)?;
	with_mix_mut(src, move |mix| {
		mix.adjust_moles(gas_id, amt);
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/scrub_into")]
fn _scrub_into_hook(into_mix: Value, gas_list: Value) {
	let gases_to_scrub = gas_list.as_list()?;
	let gas_ids: Vec<usize> = (1..gases_to_scrub.len() + 1)
		.map(|i| gas_id_from_type(&gases_to_scrub.get(i).unwrap()).unwrap())
		.collect();
	with_mixes_mut(src, into_mix, move |src_gas, dest_gas| {
		let mut buffer_alloc =
			gas::gas_mixture::GasMixture::new_from_arena(gas::constants::CELL_VOLUME);
		buffer_alloc.with_mut(|buffer| {
			buffer.set_temperature(src_gas.get_temperature());
			for &gas_id in &gas_ids {
				buffer.set_moles(gas_id, src_gas.get_moles(gas_id));
				src_gas.set_moles(gas_id, 0.0);
			}
			dest_gas.merge(&buffer);
		});
		Ok(Value::from(true))
	})
}

#[hook("/datum/gas_mixture/proc/mark_immutable")]
fn _mark_immutable_hook() {
	with_mix_mut(src, move |mix| {
		mix.mark_immutable();
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/clear")]
fn _clear_hook() {
	with_mix_mut(src, move |mix| {
		mix.clear();
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/compare")]
fn _compare_hook() {
	if args.is_empty() {
		Err(runtime!("Tried comparing a gas mix to nothing"))
	} else {
		with_mixes(src, &args[0], |gas_one, gas_two| {
			if gas_one.temperature_compare(gas_two)
				|| gas_one.compare(gas_two) > MINIMUM_MOLES_DELTA_TO_MOVE
			{
				Ok(Value::from(1.0))
			} else {
				Ok(Value::from(0.0))
			}
		})
	}
}

#[hook("/datum/gas_mixture/proc/multiply")]
fn _multiply_hook(v: Value) {
	let mult = v.as_number().unwrap_or(1.0);
	with_mix_mut(src, move |mix| {
		mix.multiply(mult);
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/react")]
fn _react_hook() {
	let mut ret: i32 = 0;
	let n = Value::null();
	let holder = args.first().unwrap_or(&n);
	let reactions = with_mix(src, |mix| Ok(mix.all_reactable()))?;
	for reaction in reactions {
		ret |= react_by_id(reaction, src, holder)?.as_number()? as i32;
		if ret & STOP_REACTIONS == STOP_REACTIONS {
			return Ok(Value::from(ret as f32));
		}
	}
	Ok(Value::from(ret as f32))
}

#[hook("/datum/gas_mixture/proc/adjust_heat")]
fn _adjust_heat_hook(v: Value) {
	let heat = v.as_number()?;
	with_mix_mut(src, move |mix| {
		mix.adjust_heat(heat);
		Ok(Value::null())
	})
}

#[hook("/datum/gas_mixture/proc/equalize_with")]
fn _equalize_with_hook() {
	with_mixes_mut(
		src,
		args.get(0)
			.ok_or_else(|| runtime!("Wrong number of args for equalize_with: 0"))?,
		|src_gas, total_gas| {
			let vol = src_gas.volume;
			src_gas.copy_from_mutable(total_gas);
			src_gas.multiply(vol / total_gas.volume);
			Ok(Value::null())
		},
	)
}

#[hook("/proc/equalize_all_gases_in_list")]
fn _equalize_all_hook() {
	use std::collections::BTreeSet;
	let value_list = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of args for equalize all: 0"))?
		.as_list()?;
	let gas_list: BTreeSet<usize> = (1..value_list.len() + 1)
		.map(|i| {
			value_list
				.get(i)
				.unwrap()
				.get_number(byond_string!("_extools_pointer_gasmixture"))
				.unwrap()
				.to_bits() as usize
		})
		.collect(); // collect because get_number is way slower than the one-time allocation
	let mut tot_alloc = gas::gas_mixture::GasMixture::new_from_arena(1.0);
	let mut tot_vol: f64 = 0.0;
	GasMixtures::with_all_mixtures(move |all_mixtures| {
		tot_alloc.with_mut(|tot| {
			for &id in gas_list.iter() {
				if let Some(src_gas) = all_mixtures.get(id) {
					tot.merge(&src_gas);
					tot_vol += src_gas.volume as f64;
				}
			}
		});
		if tot_vol > 0.0 {
			tot_alloc.with(|tot| {
				for &id in gas_list.iter() {
					if let Some(dest_gas) = all_mixtures.get_mut(id) {
						let vol = dest_gas.volume; // don't wanna borrow it in the below
						dest_gas.copy_from_mutable(&tot);
						dest_gas.multiply((vol as f64 / tot_vol) as f32);
					}
				}
			});
		}
	});
	Ok(Value::null())
}

#[hook("/proc/release_gas_to")]
fn _release_gas_to(in_gas: Value, out_gas: Value, target_pressure_v: Value) {
	let target_pressure = target_pressure_v.as_number()?;
	if let Some(transfer_moles) = with_mixes(&in_gas, &out_gas, |input_mix, output_mix| {
		let input_pressure = input_mix.return_pressure();
		let output_pressure = output_mix.return_pressure();
		if output_pressure >= target_pressure.min(input_pressure - 10.0)
			|| !(input_mix.total_moles().is_normal() && input_mix.get_temperature().is_normal())
		{
			Ok(None)
		} else {
			let pressure_delta =
				(target_pressure - output_pressure).min((input_pressure - output_pressure) / 2.0);
			Ok(Some(
				(pressure_delta * output_mix.volume)
					/ (input_mix.get_temperature() * R_IDEAL_GAS_EQUATION),
			))
		}
	})
	.unwrap()
	{
		with_mixes_mut(&in_gas, &out_gas, move |input_mix, output_mix| {
			let mut removed_gas_alloc =
				gas::gas_mixture::GasMixture::new_from_arena(input_mix.volume);
			removed_gas_alloc.with_mut(|removed_gas| {
				input_mix.remove(transfer_moles, removed_gas);
				output_mix.merge(&removed_gas);
			});
			Ok(Value::from(true))
		})
	} else {
		Ok(Value::from(false))
	}
}

#[hook("/datum/controller/subsystem/air/proc/get_amt_gas_mixes")]
fn _hook_amt_gas_mixes() {
	Ok(Value::from(amt_gases() as f32))
}

#[hook("/datum/controller/subsystem/air/proc/get_max_gas_mixes")]
fn _hook_max_gas_mixes() {
	Ok(Value::from(tot_gases() as f32))
}
