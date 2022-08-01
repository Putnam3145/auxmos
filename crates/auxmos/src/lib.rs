mod gas;

#[cfg(feature = "turf_processing")]
mod turfs;

mod reaction;

mod parser;

use auxtools::{byond_string, hook, inventory, runtime, List, Value};

use auxcleanup::{datum_del, DelDatumFunc};

use gas::{
	amt_gases, constants, gas_idx_from_string, gas_idx_from_value, gas_idx_to_id, tot_gases, types,
	with_gas_info, with_mix, with_mix_mut, with_mixes, with_mixes_custom, with_mixes_mut, GasArena,
	Mixture,
};

use reaction::react_by_id;

use gas::constants::{ReactionReturn, GAS_MIN_MOLES, MINIMUM_MOLES_DELTA_TO_MOVE};

/// Args: (ms). Runs callbacks until time limit is reached. If time limit is omitted, runs all callbacks.
#[hook("/proc/process_atmos_callbacks")]
fn _atmos_callback_handle() {
	auxcallback::callback_processing_hook(&mut args)
}

/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument Value to point to it.
#[hook("/datum/gas_mixture/proc/__gasmixture_register")]
fn _register_gasmixture_hook() {
	gas::GasArena::register_mix(src)
}

/// Adds the gas mixture's ID to the queue of mixtures that have been deleted, to be reused later.
/// This version is only if auxcleanup is not being used; it should be called from /datum/gas_mixture/Del.
#[cfg(not(feature = "auxcleanup_gas_deletion"))]
#[hook("/datum/gas_mixture/proc/__gasmixture_unregister")]
fn _unregister_gasmixture_hook() {
	gas::GasMixtures::unregister_mix(unsafe { src.raw.data.id });
	Ok(Value::null())
}

/// Adds the gas mixture's ID to the queue of mixtures that have been deleted, to be reused later. Called automatically on all datum deletion.
#[cfg(feature = "auxcleanup_gas_deletion")]
#[datum_del]
fn _unregister_gasmixture_hook(v: u32) {
	gas::GasArena::unregister_mix(v);
}

/// Returns: Heat capacity, in J/K (probably).
#[hook("/datum/gas_mixture/proc/heat_capacity")]
fn _heat_cap_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.heat_capacity())))
}

/// Args: (min_heat_cap). Sets the mix's minimum heat capacity.
#[hook("/datum/gas_mixture/proc/set_min_heat_capacity")]
fn _min_heat_cap_hook(arg_min: Value) {
	let min = arg_min.as_number()?;
	with_mix_mut(src, |mix| {
		mix.set_min_heat_capacity(min);
		Ok(Value::null())
	})
}

/// Returns: Amount of substance, in moles.
#[hook("/datum/gas_mixture/proc/total_moles")]
fn _total_moles_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.total_moles())))
}

/// Returns: the mix's pressure, in kilopascals.
#[hook("/datum/gas_mixture/proc/return_pressure")]
fn _return_pressure_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.return_pressure())))
}

/// Returns: the mix's temperature, in kelvins.
#[hook("/datum/gas_mixture/proc/return_temperature")]
fn _return_temperature_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.get_temperature())))
}

/// Returns: the mix's volume, in liters.
#[hook("/datum/gas_mixture/proc/return_volume")]
fn _return_volume_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.volume)))
}

/// Returns: the mix's thermal energy, the product of the mixture's heat capacity and its temperature.
#[hook("/datum/gas_mixture/proc/thermal_energy")]
fn _thermal_energy_hook() {
	with_mix(src, |mix| Ok(Value::from(mix.thermal_energy())))
}

/// Args: (mixture). Merges the gas from the giver into src, without modifying the giver mix.
#[hook("/datum/gas_mixture/proc/merge")]
fn _merge_hook(giver: Value) {
	with_mixes_custom(src, giver, |src_mix, giver_mix| {
		src_mix.write().merge(&giver_mix.read());
		Ok(Value::null())
	})
}

/// Args: (mixture, ratio). Takes the given ratio of gas from src and puts it into the argument mixture. Ratio is a number between 0 and 1.
#[hook("/datum/gas_mixture/proc/__remove_ratio")]
fn _remove_ratio_hook(into: Value, ratio_arg: Value) {
	let ratio = ratio_arg.as_number().unwrap_or_default();
	with_mixes_mut(src, into, |src_mix, into_mix| {
		src_mix.remove_ratio_into(ratio, into_mix);
		Ok(Value::null())
	})
}

/// Args: (mixture, amount). Takes the given amount of gas from src and puts it into the argument mixture. Amount is amount of substance in moles.
#[hook("/datum/gas_mixture/proc/__remove")]
fn _remove_hook(into: Value, amount_arg: Value) {
	let amount = amount_arg.as_number().unwrap_or_default();
	with_mixes_mut(src, into, |src_mix, into_mix| {
		src_mix.remove_into(amount, into_mix);
		Ok(Value::null())
	})
}

/// Arg: (mixture). Makes src into a copy of the argument mixture.
#[hook("/datum/gas_mixture/proc/copy_from")]
fn _copy_from_hook(giver: Value) {
	with_mixes_custom(src, giver, |src_mix, giver_mix| {
		src_mix.write().copy_from_mutable(&giver_mix.read());
		Ok(Value::null())
	})
}

/// Args: (mixture, conductivity) or (null, conductivity, temperature, heat_capacity). Adjusts temperature of src based on parameters. Returns: temperature of sharer after sharing is complete.
#[hook("/datum/gas_mixture/proc/temperature_share")]
fn _temperature_share_hook() {
	let arg_num = args.len();
	match arg_num {
		2 => with_mixes_mut(src, &args[0], |src_mix, share_mix| {
			Ok(Value::from(src_mix.temperature_share(
				share_mix,
				args[1].as_number().unwrap_or_default(),
			)))
		}),
		4 => with_mix_mut(src, |mix| {
			Ok(Value::from(mix.temperature_share_non_gas(
				args[1].as_number().unwrap_or_default(),
				args[2].as_number().unwrap_or_default(),
				args[3].as_number().unwrap_or_default(),
			)))
		}),
		_ => Err(runtime!("Invalid args for temperature_share")),
	}
}

/// Returns: a list of the gases in the mixture, associated with their IDs.
#[hook("/datum/gas_mixture/proc/get_gases")]
fn _get_gases_hook() {
	with_mix(src, |mix| {
		let gases_list: List = List::new();
		mix.for_each_gas(|idx, gas| {
			if gas > GAS_MIN_MOLES {
				gases_list.append(gas_idx_to_id(idx)?);
			}
			Ok(())
		})?;
		Ok(Value::from(gases_list))
	})
}

/// Args: (temperature). Sets the temperature of the mixture. Will be set to 2.7 if it's too low.
#[hook("/datum/gas_mixture/proc/set_temperature")]
fn _set_temperature_hook(arg_temp: Value) {
	let v = arg_temp.as_number().map_err(|_| {
		runtime!(
			"Attempt to interpret non-number value as number {} {}:{}",
			std::file!(),
			std::line!(),
			std::column!()
		)
	})?;
	if v.is_finite() {
		with_mix_mut(src, |mix| {
			mix.set_temperature(v.max(2.7));
			Ok(Value::null())
		})
	} else {
		Err(runtime!(
			"Attempted to set a temperature to a number that is NaN or infinite."
		))
	}
}

/// Args: (gas_id). Returns the heat capacity from the given gas, in J/K (probably).
#[hook("/datum/gas_mixture/proc/partial_heat_capacity")]
fn _partial_heat_capacity(gas_id: Value) {
	with_mix(src, |mix| {
		Ok(Value::from(
			mix.partial_heat_capacity(gas_idx_from_value(gas_id)?),
		))
	})
}

/// Args: (volume). Sets the volume of the gas.
#[hook("/datum/gas_mixture/proc/set_volume")]
fn _set_volume_hook(vol_arg: Value) {
	let volume = vol_arg.as_number().map_err(|_| {
		runtime!(
			"Attempt to interpret non-number value as number {} {}:{}",
			std::file!(),
			std::line!(),
			std::column!()
		)
	})?;
	with_mix_mut(src, |mix| {
		mix.volume = volume;
		Ok(Value::null())
	})
}

/// Args: (gas_id). Returns: the amount of substance of the given gas, in moles.
#[hook("/datum/gas_mixture/proc/get_moles")]
fn _get_moles_hook(gas_id: Value) {
	with_mix(src, |mix| {
		Ok(Value::from(mix.get_moles(gas_idx_from_value(gas_id)?)))
	})
}

/// Args: (gas_id, moles). Sets the amount of substance of the given gas, in moles.
#[hook("/datum/gas_mixture/proc/set_moles")]
fn _set_moles_hook(gas_id: Value, amt_val: Value) {
	let vf = amt_val.as_number()?;
	if !vf.is_finite() {
		return Err(runtime!("Attempted to set moles to NaN or infinity."));
	}
	if vf < 0.0 {
		return Err(runtime!("Attempted to set moles to a negative number."));
	}
	with_mix_mut(src, |mix| {
		mix.set_moles(gas_idx_from_value(gas_id)?, vf);
		Ok(Value::null())
	})
}
/// Args: (gas_id, moles). Adjusts the given gas's amount by the given amount, e.g. (GAS_O2, -0.1) will remove 0.1 moles of oxygen from the mixture.
#[hook("/datum/gas_mixture/proc/adjust_moles")]
fn _adjust_moles_hook(id_val: Value, num_val: Value) {
	let vf = num_val.as_number().unwrap_or_default();
	with_mix_mut(src, |mix| {
		mix.adjust_moles(gas_idx_from_value(id_val)?, vf);
		Ok(Value::null())
	})
}

/// Args: (gas_id, moles, temp). Adjusts the given gas's amount by the given amount, with that gas being treated as if it is at the given temperature.
#[hook("/datum/gas_mixture/proc/adjust_moles_temp")]
fn _adjust_moles_temp_hook(id_val: Value, num_val: Value, temp_val: Value) {
	let vf = num_val.as_number().unwrap_or_default();
	let temp = temp_val.as_number().unwrap_or(2.7);
	if vf < 0.0 {
		return Err(runtime!(
			"Attempted to add a negative gas in adjust_moles_temp."
		));
	}
	if !vf.is_normal() {
		return Ok(Value::null());
	}
	let mut new_mix = Mixture::new();
	new_mix.set_moles(gas_idx_from_value(id_val)?, vf);
	new_mix.set_temperature(temp);
	with_mix_mut(src, |mix| {
		mix.merge(&new_mix);
		Ok(Value::null())
	})
}

/// Args: (gas_id_1, amount_1, gas_id_2, amount_2, ...). As adjust_moles, but with variadic arguments.
#[hook("/datum/gas_mixture/proc/adjust_multi")]
fn _adjust_multi_hook() {
	if args.len() % 2 != 0 {
		Err(runtime!(
			"Incorrect arg len for adjust_multi (not divisible by 2)."
		))
	} else {
		let adjustments = args
			.chunks(2)
			.filter_map(|chunk| {
				(chunk.len() == 2)
					.then(|| {
						gas_idx_from_value(&chunk[0])
							.ok()
							.map(|idx| (idx, chunk[1].as_number().unwrap_or_default()))
					})
					.flatten()
			})
			.collect::<Vec<_>>();
		with_mix_mut(src, |mix| {
			mix.adjust_multi(&adjustments);
			Ok(Value::null())
		})
	}
}

///Args: (amount). Adds the given amount to each gas.
#[hook("/datum/gas_mixture/proc/add")]
fn _add_hook(num_val: Value) {
	let vf = num_val.as_number().unwrap_or_default();
	with_mix_mut(src, |mix| {
		mix.add(vf);
		Ok(Value::null())
	})
}

///Args: (amount). Subtracts the given amount from each gas.
#[hook("/datum/gas_mixture/proc/subtract")]
fn _subtract_hook(num_val: Value) {
	let vf = num_val.as_number().unwrap_or_default();
	with_mix_mut(src, |mix| {
		mix.add(-vf);
		Ok(Value::null())
	})
}

///Args: (coefficient). Multiplies all gases by this amount.
#[hook("/datum/gas_mixture/proc/multiply")]
fn _multiply_hook(num_val: Value) {
	let vf = num_val.as_number().unwrap_or(1.0);
	with_mix_mut(src, |mix| {
		mix.multiply(vf);
		Ok(Value::null())
	})
}

///Args: (coefficient). Divides all gases by this amount.
#[hook("/datum/gas_mixture/proc/divide")]
fn _divide_hook(num_val: Value) {
	let vf = num_val.as_number().unwrap_or(1.0).recip();
	with_mix_mut(src, |mix| {
		mix.multiply(vf);
		Ok(Value::null())
	})
}

///Args: (mixture, flag, amount). Takes `amount` from src that have the given `flag` and puts them into the given `mixture`. Returns: 0 if gas didn't have any with that flag, 1 if it did.
#[hook("/datum/gas_mixture/proc/__remove_by_flag")]
fn _remove_by_flag_hook(into: Value, flag_val: Value, amount_val: Value) {
	let flag = flag_val.as_number().map_or(0, |n| n as u32);
	let amount = amount_val.as_number().unwrap_or(0.0);
	let pertinent_gases = with_gas_info(|gas_info| {
		gas_info
			.iter()
			.filter(|g| g.flags & flag != 0)
			.map(|g| g.idx)
			.collect::<Vec<_>>()
	});
	if pertinent_gases.is_empty() {
		return Ok(Value::from(false));
	}
	with_mixes_mut(src, into, |src_gas, dest_gas| {
		let tot = src_gas.total_moles();
		src_gas.transfer_gases_to(amount / tot, &pertinent_gases, dest_gas);
		Ok(Value::from(true))
	})
}
///Args: (flag). As get_gases(), but only returns gases with the given flag.
#[hook("/datum/gas_mixture/proc/get_by_flag")]
fn get_by_flag_hook(flag_val: Value) {
	let flag = flag_val.as_number().map_or(0, |n| n as u32);
	let pertinent_gases = with_gas_info(|gas_info| {
		gas_info
			.iter()
			.filter(|g| g.flags & flag != 0)
			.map(|g| g.idx)
			.collect::<Vec<_>>()
	});
	if pertinent_gases.is_empty() {
		return Ok(Value::from(0.0));
	}
	with_mix(src, |mix| {
		Ok(Value::from(
			pertinent_gases
				.iter()
				.fold(0.0, |acc, idx| acc + mix.get_moles(*idx)),
		))
	})
}

/// Args: (mixture, ratio, gas_list). Takes gases given by `gas_list` and moves `ratio` amount of those gases from `src` into `mixture`.
#[hook("/datum/gas_mixture/proc/scrub_into")]
fn _scrub_into_hook(into: Value, ratio_v: Value, gas_list: Value) {
	let ratio = ratio_v.as_number().map_err(|_| {
		runtime!(
			"Attempt to interpret non-number value as number {} {}:{}",
			std::file!(),
			std::line!(),
			std::column!()
		)
	})?;
	let gases_to_scrub = gas_list.as_list().map_err(|_| {
		runtime!(
			"Attempt to interpret non-list value as list {} {}:{}",
			std::file!(),
			std::line!(),
			std::column!()
		)
	})?;
	if gases_to_scrub.len() == 0 {
		return Ok(Value::from(false));
	}
	let gas_scrub_vec = (1..=gases_to_scrub.len())
		.filter_map(|idx| gas_idx_from_value(&gases_to_scrub.get(idx).unwrap()).ok())
		.collect::<Vec<_>>();
	with_mixes_mut(src, into, |src_gas, dest_gas| {
		src_gas.transfer_gases_to(ratio, &gas_scrub_vec, dest_gas);
		Ok(Value::from(true))
	})
}

/// Marks the mix as immutable, meaning it will never change. This cannot be undone.
#[hook("/datum/gas_mixture/proc/mark_immutable")]
fn _mark_immutable_hook() {
	with_mix_mut(src, |mix| {
		mix.mark_immutable();
		Ok(Value::null())
	})
}

/// Clears the gas mixture my removing all of its gases.
#[hook("/datum/gas_mixture/proc/clear")]
fn _clear_hook() {
	with_mix_mut(src, |mix| {
		mix.clear();
		Ok(Value::null())
	})
}

/// Returns: true if the two mixtures are different enough for processing, false otherwise.
#[hook("/datum/gas_mixture/proc/compare")]
fn _compare_hook(other: Value) {
	with_mixes(src, other, |gas_one, gas_two| {
		Ok(Value::from(
			gas_one.temperature_compare(gas_two)
				|| gas_one.compare_with(gas_two, MINIMUM_MOLES_DELTA_TO_MOVE),
		))
	})
}

/// Args: (holder). Runs all reactions on this gas mixture. Holder is used by the reactions, and can be any arbitrary datum or null.
#[hook("/datum/gas_mixture/proc/react")]
fn _react_hook(holder: Value) {
	let mut ret = ReactionReturn::NO_REACTION;
	let reactions = with_mix(src, |mix| Ok(mix.all_reactable()))?;
	for reaction in reactions {
		ret |= ReactionReturn::from_bits_truncate(
			react_by_id(reaction, src, holder)?
				.as_number()
				.unwrap_or_default() as u32,
		);
		if ret.contains(ReactionReturn::STOP_REACTIONS) {
			return Ok(Value::from(ret.bits() as f32));
		}
	}
	Ok(Value::from(ret.bits() as f32))
}

/// Args: (heat). Adds a given amount of heat to the mixture, i.e. in joules taking into account capacity.
#[hook("/datum/gas_mixture/proc/adjust_heat")]
fn _adjust_heat_hook() {
	with_mix_mut(src, |mix| {
		mix.adjust_heat(
			args.get(0)
				.ok_or_else(|| runtime!("Wrong number of args for adjust heat: 0"))?
				.as_number()
				.map_err(|_| {
					runtime!(
						"Attempt to interpret non-number value as number {} {}:{}",
						std::file!(),
						std::line!(),
						std::column!()
					)
				})?,
		);
		Ok(Value::null())
	})
}

/// Args: (mixture, amount). Takes the `amount` given and transfers it from `src` to `mixture`.
#[hook("/datum/gas_mixture/proc/transfer_to")]
fn _transfer_hook(other: Value, moles: Value) {
	with_mixes_mut(src, other, |our_mix, other_mix| {
		other_mix.merge(&our_mix.remove(moles.as_number().map_err(|_| {
			runtime!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})?));
		Ok(Value::null())
	})
}

/// Args: (mixture, ratio). Transfers `ratio` of `src` to `mixture`.
#[hook("/datum/gas_mixture/proc/transfer_ratio_to")]
fn _transfer_ratio_hook(other: Value, ratio: Value) {
	with_mixes_mut(src, other, |our_mix, other_mix| {
		other_mix.merge(&our_mix.remove_ratio(ratio.as_number().map_err(|_| {
			runtime!(
				"Attempt to interpret non-number value as number {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})?));
		Ok(Value::null())
	})
}

/// Args: (mixture). Makes `src` a copy of `mixture`, with volumes taken into account.
#[hook("/datum/gas_mixture/proc/equalize_with")]
fn _equalize_with_hook(total: Value) {
	with_mixes_custom(src, total, |src_lock, total_lock| {
		let src_gas = &mut src_lock.write();
		let vol = src_gas.volume;
		let total_gas = total_lock.read();
		src_gas.copy_from_mutable(&total_gas);
		src_gas.multiply(vol / total_gas.volume);
		Ok(Value::null())
	})
}

/// Args: (temperature). Returns: how much fuel for fire is in the mixture at the given temperature. If temperature is omitted, just uses current temperature instead.
#[hook("/datum/gas_mixture/proc/get_fuel_amount")]
fn _fuel_amount_hook(temp: Value) {
	with_mix(src, |air| {
		Ok(Value::from(temp.as_number().ok().map_or_else(
			|| air.get_fuel_amount(),
			|new_temp| {
				let mut test_air = air.clone();
				test_air.set_temperature(new_temp);
				test_air.get_fuel_amount()
			},
		)))
	})
}

/// Args: (temperature). Returns: how much oxidizer for fire is in the mixture at the given temperature. If temperature is omitted, just uses current temperature instead.
#[hook("/datum/gas_mixture/proc/get_oxidation_power")]
fn _oxidation_power_hook(temp: Value) {
	with_mix(src, |air| {
		Ok(Value::from(temp.as_number().ok().map_or_else(
			|| air.get_oxidation_power(),
			|new_temp| {
				let mut test_air = air.clone();
				test_air.set_temperature(new_temp);
				test_air.get_oxidation_power()
			},
		)))
	})
}

/// Args: (mixture, ratio, one_way). Shares the given `ratio` of `src` with `mixture`, and, unless `one_way` is truthy, vice versa.
#[cfg(feature = "zas_hooks")]
#[hook("/datum/gas_mixture/proc/share_ratio")]
fn _share_ratio_hook(other_gas: Value, ratio_val: Value, one_way_val: Value) {
	let one_way = one_way_val.as_bool().unwrap_or(false);
	let ratio = ratio_val.as_number().ok().map_or(0.6);
	let mut inbetween = Mixture::new();
	if one_way {
		with_mixes_custom(src, other_gas, |src_lock, other_lock| {
			let src_mix = src_lock.write();
			let other_mix = other_lock.read();
			inbetween.copy_from_mutable(other_mix);
			inbetween.multiply(ratio);
			inbetween.merge(&src_mix.remove_ratio(ratio));
			inbetween.multiply(0.5);
			src_mix.merge(inbetween);
			Ok(Value::from(
				src_mix.temperature_compare(other_mix)
					|| src_mix.compare_with(other_mix, MINIMUM_MOLES_DELTA_TO_MOVE),
			))
		})
	} else {
		with_mixes_mut(src, other_gas, |src_mix, other_mix| {
			src_mix.remove_ratio_into(ratio, &mut inbetween);
			inbetween.merge(&other_mix.remove_ratio(ratio));
			inbetween.multiply(0.5);
			src_mix.merge(inbetween);
			other_mix.merge(inbetween);
			Ok(Value::from(
				src_mix.temperature_compare(other_mix)
					|| src_mix.compare_with(other_mix, MINIMUM_MOLES_DELTA_TO_MOVE),
			))
		})
	}
}

/// Args: (list). Takes every gas in the list and makes them all identical, scaled to their respective volumes. The total heat and amount of substance in all of the combined gases is conserved.
#[hook("/proc/equalize_all_gases_in_list")]
fn _equalize_all_hook() {
	use std::collections::BTreeSet;
	let value_list = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of args for equalize all: 0"))?
		.as_list()
		.map_err(|_| {
			runtime!(
				"Attempt to interpret non-list value as list {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})?;
	let gas_list: BTreeSet<usize> = (1..=value_list.len())
		.filter_map(|i| {
			value_list
				.get(i)
				.unwrap_or_else(|_| Value::null())
				.get_number(byond_string!("_extools_pointer_gasmixture"))
				.ok()
				.map(|f| f.to_bits() as usize)
		})
		.collect(); // collect because get_number is way slower than the one-time allocation
	GasArena::with_all_mixtures(move |all_mixtures| {
		let mut tot = gas::Mixture::new();
		let mut tot_vol: f64 = 0.0;
		for &id in &gas_list {
			if let Some(src_gas_lock) = all_mixtures.get(id) {
				let src_gas = src_gas_lock.read();
				tot.merge(&src_gas);
				tot_vol += f64::from(src_gas.volume);
			}
		}
		if tot_vol > 0.0 {
			for &id in &gas_list {
				if let Some(dest_gas_lock) = all_mixtures.get(id) {
					let dest_gas = &mut dest_gas_lock.write();
					let vol = dest_gas.volume; // don't wanna borrow it in the below
					dest_gas.copy_from_mutable(&tot);
					dest_gas.multiply((f64::from(vol) / tot_vol) as f32);
				}
			}
		}
	});
	Ok(Value::null())
}

/// Returns: the amount of gas mixtures that are attached to a byond gas mixture.
#[hook("/datum/controller/subsystem/air/proc/get_amt_gas_mixes")]
fn _hook_amt_gas_mixes() {
	Ok(Value::from(amt_gases() as f32))
}

/// Returns: the total amount of gas mixtures in the arena, including "free" ones.
#[hook("/datum/controller/subsystem/air/proc/get_max_gas_mixes")]
fn _hook_max_gas_mixes() {
	Ok(Value::from(tot_gases() as f32))
}

#[hook("/datum/gas_mixture/proc/__auxtools_parse_gas_string")]
fn _parse_gas_string(string: Value) {
	let actual_string = string.as_string()?;

	let (_, vec) = parser::parse_gas_string(&actual_string)
		.map_err(|_| runtime!(format!("Failed to parse gas string: {}", actual_string)))?;

	with_mix_mut(src, move |air| {
		air.clear();
		for (gas, moles) in vec.iter() {
			if let Ok(idx) = gas_idx_from_string(gas) {
				if (*moles).is_normal() && *moles > 0.0 {
					air.set_moles(idx, *moles)
				}
			} else if gas.contains("TEMP") {
				let mut checked_temp = *moles;
				if !checked_temp.is_normal() || checked_temp < constants::TCMB {
					checked_temp = constants::TCMB
				}
				air.set_temperature(checked_temp)
			} else {
				return Err(runtime!(format!("Unknown gas id: {}", gas)));
			}
		}
		Ok(())
	})?;
	Ok(Value::from(true))
}
