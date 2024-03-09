pub mod gas;

#[cfg(feature = "turf_processing")]
pub mod turfs;

mod reaction;

mod parser;

use byondapi::{map::byond_length, prelude::*};

use gas::{
	amt_gases, constants, gas_idx_from_string, gas_idx_from_value, gas_idx_to_id, tot_gases, types,
	with_gas_info, with_mix, with_mix_mut, with_mixes, with_mixes_custom, with_mixes_mut, GasArena,
	Mixture,
};

use reaction::react_by_id;

use gas::constants::{ReactionReturn, GAS_MIN_MOLES, MINIMUM_MOLES_DELTA_TO_MOVE};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[byondapi::init]
pub fn init_eyre() {
	stable_eyre::install().unwrap();
}

/// Args: (ms). Runs callbacks until time limit is reached. If time limit is omitted, runs all callbacks.
#[byondapi::bind("/proc/process_atmos_callbacks")]
fn atmos_callback_handle(remaining: ByondValue) {
	auxcallback::callback_processing_hook(remaining)
}

/// Fills in the first unused slot in the gas mixtures vector, or adds another one, then sets the argument ByondValue to point to it.
#[byondapi::bind("/datum/gas_mixture/proc/__gasmixture_register")]
fn register_gasmixture_hook(src: ByondValue) {
	gas::GasArena::register_mix(src)
}

/// Adds the gas mixture's ID to the queue of mixtures that have been deleted, to be reused later.
/// This version is only if auxcleanup is not being used; it should be called from /datum/gas_mixture/Del.
#[byondapi::bind("/datum/gas_mixture/proc/__gasmixture_unregister")]
fn unregister_gasmixture_hook(src: ByondValue) {
	gas::GasArena::unregister_mix(&src);
	Ok(ByondValue::null())
}

/// Returns: Heat capacity, in J/K (probably).
#[byondapi::bind("/datum/gas_mixture/proc/heat_capacity")]
fn heat_cap_hook(src: ByondValue) {
	with_mix(&src, |mix| Ok(mix.heat_capacity().into()))
}

/// Args: (min_heat_cap). Sets the mix's minimum heat capacity.
#[byondapi::bind("/datum/gas_mixture/proc/set_min_heat_capacity")]
fn min_heat_cap_hook(src: ByondValue, arg_min: ByondValue) {
	let min = arg_min.get_number()?;
	with_mix_mut(&src, |mix| {
		mix.set_min_heat_capacity(min);
		Ok(ByondValue::null())
	})
}

/// Returns: Amount of substance, in moles.
#[byondapi::bind("/datum/gas_mixture/proc/total_moles")]
fn total_moles_hook(src: ByondValue) {
	Ok(with_mix(&src, |mix| Ok(mix.total_moles().into()))?)
}

/// Returns: the mix's pressure, in kilopascals.
#[byondapi::bind("/datum/gas_mixture/proc/return_pressure")]
fn return_pressure_hook(src: ByondValue) {
	with_mix(&src, |mix| Ok(mix.return_pressure().into()))
}

/// Returns: the mix's temperature, in kelvins.
#[byondapi::bind("/datum/gas_mixture/proc/return_temperature")]
fn return_temperature_hook(src: ByondValue) {
	with_mix(&src, |mix| Ok(mix.get_temperature().into()))
}

/// Returns: the mix's volume, in liters.
#[byondapi::bind("/datum/gas_mixture/proc/return_volume")]
fn return_volume_hook(src: ByondValue) {
	with_mix(&src, |mix| Ok(mix.volume.into()))
}

/// Returns: the mix's thermal energy, the product of the mixture's heat capacity and its temperature.
#[byondapi::bind("/datum/gas_mixture/proc/thermal_energy")]
fn thermal_energy_hook(src: ByondValue) {
	with_mix(&src, |mix| Ok(mix.thermal_energy().into()))
}

/// Args: (mixture). Merges the gas from the giver into src, without modifying the giver mix.
#[byondapi::bind("/datum/gas_mixture/proc/merge")]
fn merge_hook(src: ByondValue, giver: ByondValue) {
	with_mixes_custom(&src, &giver, |src_mix, giver_mix| {
		src_mix.write().merge(&giver_mix.read());
		Ok(ByondValue::null())
	})
}

/// Args: (mixture, ratio). Takes the given ratio of gas from src and puts it into the argument mixture. Ratio is a number between 0 and 1.
#[byondapi::bind("/datum/gas_mixture/proc/__remove_ratio")]
fn remove_ratio_hook(src: ByondValue, into: ByondValue, ratio_arg: ByondValue) {
	let ratio = ratio_arg.get_number().unwrap_or_default();
	with_mixes_mut(&src, &into, |src_mix, into_mix| {
		src_mix.remove_ratio_into(ratio, into_mix);
		Ok(ByondValue::null())
	})
}

/// Args: (mixture, amount). Takes the given amount of gas from src and puts it into the argument mixture. Amount is amount of substance in moles.
#[byondapi::bind("/datum/gas_mixture/proc/__remove")]
fn remove_hook(src: ByondValue, into: ByondValue, amount_arg: ByondValue) {
	let amount = amount_arg.get_number().unwrap_or_default();
	with_mixes_mut(&src, &into, |src_mix, into_mix| {
		src_mix.remove_into(amount, into_mix);
		Ok(ByondValue::null())
	})
}

/// Arg: (mixture). Makes src into a copy of the argument mixture.
#[byondapi::bind("/datum/gas_mixture/proc/copy_from")]
fn copy_from_hook(src: ByondValue, giver: ByondValue) {
	with_mixes_custom(&src, &giver, |src_mix, giver_mix| {
		src_mix.write().copy_from_mutable(&giver_mix.read());
		Ok(ByondValue::null())
	})
}

/// Args: (src, mixture, conductivity) or (src, conductivity, temperature, heat_capacity). Adjusts temperature of src based on parameters. Returns: temperature of sharer after sharing is complete.
#[byondapi::bind_raw_args("/datum/gas_mixture/proc/temperature_share")]
fn temperature_share_hook() {
	let arg_num = args.len();
	match arg_num {
		3 => with_mixes_mut(&args[0], &args[1], |src_mix, share_mix| {
			Ok(src_mix
				.temperature_share(share_mix, args[2].get_number().unwrap_or_default())
				.into())
		}),
		4 => with_mix_mut(&args[0], |mix| {
			Ok(mix
				.temperature_share_non_gas(
					args[1].get_number().unwrap_or_default(),
					args[2].get_number().unwrap_or_default(),
					args[3].get_number().unwrap_or_default(),
				)
				.into())
		}),
		_ => Err(eyre::eyre!("Invalid args for temperature_share")),
	}
}

/// Returns: a list of the gases in the mixture, associated with their IDs.
#[byondapi::bind("/datum/gas_mixture/proc/get_gases")]
fn get_gases_hook(src: ByondValue) {
	with_mix(&src, |mix| {
		let mut gases_list = ByondValue::new_list()?;
		mix.for_each_gas(|idx, gas| {
			if gas > GAS_MIN_MOLES {
				gases_list.push_list(gas_idx_to_id(idx))?;
			}
			Ok(())
		})?;

		Ok(gases_list)
	})
}

/// Args: (temperature). Sets the temperature of the mixture. Will be set to 2.7 if it's too low.
#[byondapi::bind("/datum/gas_mixture/proc/set_temperature")]
fn set_temperature_hook(src: ByondValue, arg_temp: ByondValue) {
	let v = arg_temp.get_number()?;
	if v.is_finite() {
		with_mix_mut(&src, |mix| {
			mix.set_temperature(v.max(2.7));
			Ok(ByondValue::null())
		})
	} else {
		Err(eyre::eyre!(
			"Attempted to set a temperature to a number that is NaN or infinite."
		))
	}
}

/// Args: (gas_id). Returns the heat capacity from the given gas, in J/K (probably).
#[byondapi::bind("/datum/gas_mixture/proc/partial_heat_capacity")]
fn partial_heat_capacity(src: ByondValue, gas_id: ByondValue) {
	with_mix(&src, |mix| {
		Ok(mix
			.partial_heat_capacity(gas_idx_from_value(&gas_id)?)
			.into())
	})
}

/// Args: (volume). Sets the volume of the gas.
#[byondapi::bind("/datum/gas_mixture/proc/set_volume")]
fn set_volume_hook(src: ByondValue, vol_arg: ByondValue) {
	let volume = vol_arg.get_number()?;
	with_mix_mut(&src, |mix| {
		mix.volume = volume;
		Ok(ByondValue::null())
	})
}

/// Args: (gas_id). Returns: the amount of substance of the given gas, in moles.
#[byondapi::bind("/datum/gas_mixture/proc/get_moles")]
fn get_moles_hook(src: ByondValue, gas_id: ByondValue) {
	with_mix(&src, |mix| {
		Ok(mix.get_moles(gas_idx_from_value(&gas_id)?).into())
	})
}

/// Args: (gas_id, moles). Sets the amount of substance of the given gas, in moles.
#[byondapi::bind("/datum/gas_mixture/proc/set_moles")]
fn set_moles_hook(src: ByondValue, gas_id: ByondValue, amt_val: ByondValue) {
	let vf = amt_val.get_number()?;
	if !vf.is_finite() {
		return Err(eyre::eyre!("Attempted to set moles to NaN or infinity."));
	}
	if vf < 0.0 {
		return Err(eyre::eyre!("Attempted to set moles to a negative number."));
	}
	with_mix_mut(&src, |mix| {
		mix.set_moles(gas_idx_from_value(&gas_id)?, vf);
		Ok(ByondValue::null())
	})
}
/// Args: (gas_id, moles). Adjusts the given gas's amount by the given amount, e.g. (GAS_O2, -0.1) will remove 0.1 moles of oxygen from the mixture.
#[byondapi::bind("/datum/gas_mixture/proc/adjust_moles")]
fn adjust_moles_hook(src: ByondValue, id_val: ByondValue, num_val: ByondValue) {
	let vf = num_val.get_number().unwrap_or_default();
	with_mix_mut(&src, |mix| {
		mix.adjust_moles(gas_idx_from_value(&id_val)?, vf);
		Ok(ByondValue::null())
	})
}

/// Args: (gas_id, moles, temp). Adjusts the given gas's amount by the given amount, with that gas being treated as if it is at the given temperature.
#[byondapi::bind("/datum/gas_mixture/proc/adjust_moles_temp")]
fn adjust_moles_temp_hook(
	src: ByondValue,
	id_val: ByondValue,
	num_val: ByondValue,
	temp_val: ByondValue,
) {
	let vf = num_val.get_number().unwrap_or_default();
	let temp = temp_val.get_number().unwrap_or(2.7);
	if vf < 0.0 {
		return Err(eyre::eyre!(
			"Attempted to add a negative gas in adjust_moles_temp."
		));
	}
	if !vf.is_normal() {
		return Ok(ByondValue::null());
	}
	let mut new_mix = Mixture::new();
	new_mix.set_moles(gas_idx_from_value(&id_val)?, vf);
	new_mix.set_temperature(temp);
	with_mix_mut(&src, |mix| {
		mix.merge(&new_mix);
		Ok(ByondValue::null())
	})
}

/// Args: (gas_id_1, amount_1, gas_id_2, amount_2, ...). As adjust_moles, but with variadic arguments.
#[byondapi::bind_raw_args("/datum/gas_mixture/proc/adjust_multi")]
fn adjust_multi_hook() {
	if args.len() % 2 == 0 {
		Err(eyre::eyre!(
			"Incorrect arg len for adjust_multi (is even, must be odd to account for src)."
		))
	} else if let Some((src, rest)) = args.split_first() {
		let adjustments = rest
			.chunks(2)
			.filter_map(|chunk| {
				(chunk.len() == 2)
					.then(|| {
						gas_idx_from_value(&chunk[0])
							.ok()
							.map(|idx| (idx, chunk[1].get_number().unwrap_or_default()))
					})
					.flatten()
			})
			.collect::<Vec<_>>();
		with_mix_mut(src, |mix| {
			mix.adjust_multi(&adjustments);
			Ok(ByondValue::null())
		})
	} else {
		Err(eyre::eyre!("Invalid number of args for adjust_multi"))
	}
}

///Args: (amount). Adds the given amount to each gas.
#[byondapi::bind("/datum/gas_mixture/proc/add")]
fn add_hook(src: ByondValue, num_val: ByondValue) {
	let vf = num_val.get_number().unwrap_or_default();
	with_mix_mut(&src, |mix| {
		mix.add(vf);
		Ok(ByondValue::null())
	})
}

///Args: (amount). Subtracts the given amount from each gas.
#[byondapi::bind("/datum/gas_mixture/proc/subtract")]
fn subtract_hook(src: ByondValue, num_val: ByondValue) {
	let vf = num_val.get_number().unwrap_or_default();
	with_mix_mut(&src, |mix| {
		mix.add(-vf);
		Ok(ByondValue::null())
	})
}

///Args: (coefficient). Multiplies all gases by this amount.
#[byondapi::bind("/datum/gas_mixture/proc/multiply")]
fn multiply_hook(src: ByondValue, num_val: ByondValue) {
	let vf = num_val.get_number().unwrap_or(1.0);
	with_mix_mut(&src, |mix| {
		mix.multiply(vf);
		Ok(ByondValue::null())
	})
}

///Args: (coefficient). Divides all gases by this amount.
#[byondapi::bind("/datum/gas_mixture/proc/divide")]
fn divide_hook(src: ByondValue, num_val: ByondValue) {
	let vf = num_val.get_number().unwrap_or(1.0).recip();
	with_mix_mut(&src, |mix| {
		mix.multiply(vf);
		Ok(ByondValue::null())
	})
}

///Args: (mixture, flag, amount). Takes `amount` from src that have the given `flag` and puts them into the given `mixture`. Returns: 0 if gas didn't have any with that flag, 1 if it did.
#[byondapi::bind("/datum/gas_mixture/proc/__remove_by_flag")]
fn remove_by_flag_hook(
	src: ByondValue,
	into: ByondValue,
	flag_val: ByondValue,
	amount_val: ByondValue,
) {
	let flag = flag_val.get_number().map_or(0, |n: f32| n as u32);
	let amount = amount_val.get_number().unwrap_or(0.0);
	let pertinent_gases = with_gas_info(|gas_info| {
		gas_info
			.iter()
			.filter(|g| g.flags & flag != 0)
			.map(|g| g.idx)
			.collect::<Vec<_>>()
	});
	if pertinent_gases.is_empty() {
		return Ok(false.into());
	}
	with_mixes_mut(&src, &into, |src_gas, dest_gas| {
		let tot = src_gas.total_moles();
		src_gas.transfer_gases_to(amount / tot, &pertinent_gases, dest_gas);
		Ok(true.into())
	})
}
///Args: (flag). As get_gases(), but only returns gases with the given flag.
#[byondapi::bind("/datum/gas_mixture/proc/get_by_flag")]
fn get_by_flag_hook(src: ByondValue, flag_val: ByondValue) {
	let flag = flag_val.get_number().map_or(0, |n: f32| n as u32);
	let pertinent_gases = with_gas_info(|gas_info| {
		gas_info
			.iter()
			.filter(|g| g.flags & flag != 0)
			.map(|g| g.idx)
			.collect::<Vec<_>>()
	});
	if pertinent_gases.is_empty() {
		return Ok(0.0.into());
	}
	with_mix(&src, |mix| {
		Ok(pertinent_gases
			.iter()
			.fold(0.0, |acc, idx| acc + mix.get_moles(*idx))
			.into())
	})
}

/// Args: (mixture, ratio, gas_list). Takes gases given by `gas_list` and moves `ratio` amount of those gases from `src` into `mixture`.
#[byondapi::bind("/datum/gas_mixture/proc/scrub_into")]
fn scrub_into_hook(src: ByondValue, into: ByondValue, ratio_v: ByondValue, gas_list: ByondValue) {
	let ratio = ratio_v.get_number()?;
	if !gas_list.is_list() {
		return Err(eyre::eyre!("Non-list gas_list passed to scrub_into!"));
	}
	if byond_length(&gas_list)?.get_number()? as u32 == 0 {
		return Ok(false.into());
	}
	let gas_scrub_vec = gas_list
		.iter()?
		.filter_map(|(k, _)| gas_idx_from_value(&k).ok())
		.collect::<Vec<_>>();
	with_mixes_mut(&src, &into, |src_gas, dest_gas| {
		src_gas.transfer_gases_to(ratio, &gas_scrub_vec, dest_gas);
		Ok(true.into())
	})
}

/// Marks the mix as immutable, meaning it will never change. This cannot be undone.
#[byondapi::bind("/datum/gas_mixture/proc/mark_immutable")]
fn mark_immutable_hook(src: ByondValue) {
	with_mix_mut(&src, |mix| {
		mix.mark_immutable();
		Ok(ByondValue::null())
	})
}

/// Clears the gas mixture my removing all of its gases.
#[byondapi::bind("/datum/gas_mixture/proc/clear")]
fn clear_hook(src: ByondValue) {
	with_mix_mut(&src, |mix| {
		mix.clear();
		Ok(ByondValue::null())
	})
}

/// Returns: true if the two mixtures are different enough for processing, false otherwise.
#[byondapi::bind("/datum/gas_mixture/proc/compare")]
fn compare_hook(src: ByondValue, other: ByondValue) {
	with_mixes(&src, &other, |gas_one, gas_two| {
		Ok((gas_one.temperature_compare(gas_two)
			|| gas_one.compare_with(gas_two, MINIMUM_MOLES_DELTA_TO_MOVE))
		.into())
	})
}

/// Args: (holder). Runs all reactions on this gas mixture. Holder is used by the reactions, and can be any arbitrary datum or null.
#[byondapi::bind("/datum/gas_mixture/proc/react")]
fn react_hook(src: ByondValue, holder: ByondValue) {
	let mut ret = ReactionReturn::NO_REACTION;
	let reactions = with_mix(&src, |mix| Ok(mix.all_reactable()))?;
	for reaction in reactions {
		ret |= ReactionReturn::from_bits_truncate(
			react_by_id(reaction, src, holder)?
				.get_number()
				.unwrap_or_default() as u32,
		);
		if ret.contains(ReactionReturn::STOP_REACTIONS) {
			return Ok((ret.bits() as f32).into());
		}
	}
	Ok((ret.bits() as f32).into())
}

/// Args: (heat). Adds a given amount of heat to the mixture, i.e. in joules taking into account capacity.
#[byondapi::bind("/datum/gas_mixture/proc/adjust_heat")]
fn adjust_heat_hook(src: ByondValue, temp: ByondValue) {
	with_mix_mut(&src, |mix| {
		mix.adjust_heat(temp.get_number()?);
		Ok(ByondValue::null())
	})
}

/// Args: (mixture, amount). Takes the `amount` given and transfers it from `src` to `mixture`.
#[byondapi::bind("/datum/gas_mixture/proc/transfer_to")]
fn transfer_hook(src: ByondValue, other: ByondValue, moles: ByondValue) {
	with_mixes_mut(&src, &other, |our_mix, other_mix| {
		other_mix.merge(&our_mix.remove(moles.get_number()?));
		Ok(ByondValue::null())
	})
}

/// Args: (mixture, ratio). Transfers `ratio` of `src` to `mixture`.
#[byondapi::bind("/datum/gas_mixture/proc/transfer_ratio_to")]
fn transfer_ratio_hook(src: ByondValue, other: ByondValue, ratio: ByondValue) {
	with_mixes_mut(&src, &other, |our_mix, other_mix| {
		other_mix.merge(&our_mix.remove_ratio(ratio.get_number()?));
		Ok(ByondValue::null())
	})
}

/// Args: (mixture). Makes `src` a copy of `mixture`, with volumes taken into account.
#[byondapi::bind("/datum/gas_mixture/proc/equalize_with")]
fn equalize_with_hook(src: ByondValue, total: ByondValue) {
	with_mixes_custom(&src, &total, |src_lock, total_lock| {
		let src_gas = &mut src_lock.write();
		let vol = src_gas.volume;
		let total_gas = total_lock.read();
		src_gas.copy_from_mutable(&total_gas);
		src_gas.multiply(vol / total_gas.volume);
		Ok(ByondValue::null())
	})
}

/// Args: (temperature). Returns: how much fuel for fire is in the mixture at the given temperature. If temperature is omitted, just uses current temperature instead.
#[byondapi::bind("/datum/gas_mixture/proc/get_fuel_amount")]
fn fuel_amount_hook(src: ByondValue, temp: ByondValue) {
	with_mix(&src, |air| {
		Ok(temp
			.get_number()
			.ok()
			.map_or_else(
				|| air.get_fuel_amount(),
				|new_temp| {
					let mut test_air = air.copy_to_mutable();
					test_air.set_temperature(new_temp);
					test_air.get_fuel_amount()
				},
			)
			.into())
	})
}

/// Args: (temperature). Returns: how much oxidizer for fire is in the mixture at the given temperature. If temperature is omitted, just uses current temperature instead.
#[byondapi::bind("/datum/gas_mixture/proc/get_oxidation_power")]
fn oxidation_power_hook(src: ByondValue, temp: ByondValue) {
	with_mix(&src, |air| {
		Ok(temp
			.get_number()
			.ok()
			.map_or_else(
				|| air.get_oxidation_power(),
				|new_temp| {
					let mut test_air = air.clone();
					test_air.set_temperature(new_temp);
					test_air.get_oxidation_power()
				},
			)
			.into())
	})
}

/// Args: (mixture, ratio, one_way). Shares the given `ratio` of `src` with `mixture`, and, unless `one_way` is truthy, vice versa.
#[cfg(feature = "zas_hooks")]
#[byondapi::bind("/datum/gas_mixture/proc/share_ratio")]
fn share_ratio_hook(other_gas: ByondValue, ratio_val: ByondValue, one_way_val: ByondValue) {
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
			Ok(ByondValue::from(
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
			Ok(ByondValue::from(
				src_mix.temperature_compare(other_mix)
					|| src_mix.compare_with(other_mix, MINIMUM_MOLES_DELTA_TO_MOVE),
			))
		})
	}
}

/// Args: (list). Takes every gas in the list and makes them all identical, scaled to their respective volumes. The total heat and amount of substance in all of the combined gases is conserved.
#[byondapi::bind("/proc/equalize_all_gases_in_list")]
fn equalize_all_hook(gas_list: ByondValue) {
	use std::collections::BTreeSet;
	let gas_list = gas_list
		.iter()?
		.filter_map(|(value, _)| {
			value
				.read_number_id(byond_string!("_extools_pointer_gasmixture"))
				.ok()
				.map(|f| f as usize)
		})
		.collect::<BTreeSet<_>>();
	GasArena::with_all_mixtures(move |all_mixtures| {
		let mut tot = gas::Mixture::new();
		let mut tot_vol: f64 = 0.0;
		gas_list
			.iter()
			.filter_map(|&id| all_mixtures.get(id))
			.for_each(|src_gas_lock| {
				let src_gas = src_gas_lock.read();
				tot.merge(&src_gas);
				tot_vol += f64::from(src_gas.volume);
			});
		if tot_vol > 0.0 {
			gas_list
				.iter()
				.filter_map(|&id| all_mixtures.get(id))
				.for_each(|dest_gas_lock| {
					let dest_gas = &mut dest_gas_lock.write();
					let vol = dest_gas.volume; // don't wanna borrow it in the below
					dest_gas.copy_from_mutable(&tot);
					dest_gas.multiply((f64::from(vol) / tot_vol) as f32);
				});
		}
	});
	Ok(ByondValue::null())
}

/// Returns: the amount of gas mixtures that are attached to a byond gas mixture.
#[byondapi::bind("/datum/controller/subsystem/air/proc/get_amt_gas_mixes")]
fn hook_amt_gas_mixes() {
	Ok((amt_gases() as f32).into())
}

/// Returns: the total amount of gas mixtures in the arena, including "free" ones.
#[byondapi::bind("/datum/controller/subsystem/air/proc/get_max_gas_mixes")]
fn hook_max_gas_mixes() {
	Ok((tot_gases() as f32).into())
}

#[byondapi::bind("/datum/gas_mixture/proc/__auxtools_parse_gas_string")]
fn parse_gas_string(src: ByondValue, string: ByondValue) {
	let actual_string = string.get_string()?;

	let (_, vec) = parser::parse_gas_string(&actual_string)
		.map_err(|_| eyre::eyre!(format!("Failed to parse gas string: {actual_string}")))?;

	with_mix_mut(&src, move |air| {
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
				return Err(eyre::eyre!(format!("Unknown gas id: {gas}")));
			}
		}
		Ok(())
	})?;
	Ok(true.into())
}

#[test]
fn generate_binds() {
	byondapi::generate_bindings(env!("CARGO_CRATE_NAME"));
}
