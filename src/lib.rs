#[macro_use]
extern crate lazy_static;

mod gas;

use dm::*;

use gas::{gas_id_from_type, gas_id_to_type, get_mix, get_mix_mut};

#[hook("/datum/gas_mixture/proc/__gasmixture_register")]
fn _register_gasmixture_hook() {
	gas::GasMixtures::register_gasmix(src);
	Ok(Value::null())
}

#[hook("/datum/gas_mixture/proc/__gasmixture_unregister")]
fn _unregister_gasmixture_hook() {
	gas::GasMixtures::unregister_gasmix(src);
	Ok(Value::null())
}

#[hook("/datum/gas_mixture/proc/heat_capacity")]
fn _heat_cap_hook() {
	Ok(Value::from(get_mix(src).heat_capacity()))
}

#[hook("/datum/gas_mixture/proc/set_min_heat_capacity")]
fn _min_heat_cap_hook() {
	if args.is_empty() {
		Err(runtime!(
			"attempted to set min heat capacity with no argument"
		))
	} else {
		get_mix_mut(src).min_heat_capacity = args[0].as_number().unwrap_or(0.0);
		Ok(Value::null())
	}
}

#[hook("/datum/gas_mixture/proc/total_moles")]
fn _total_moles_hook() {
	Ok(Value::from(get_mix(src).total_moles()))
}

#[hook("/datum/gas_mixture/proc/return_pressure")]
fn _return_pressure_hook() {
	Ok(Value::from(get_mix(src).return_pressure()))
}

#[hook("/datum/gas_mixture/proc/return_temperature")]
fn _return_temperature_hook() {
	Ok(Value::from(get_mix(src).get_temperature()))
}

#[hook("/datum/gas_mixture/proc/return_volume")]
fn _return_volume_hook() {
	Ok(Value::from(get_mix(src).volume))
}

#[hook("/datum/gas_mixture/proc/thermal_energy")]
fn _thermal_energy_hook() {
	Ok(Value::from(get_mix(src).thermal_energy()))
}

#[hook("/datum/gas_mixture/proc/merge")]
fn _merge_hook() {
	if args.is_empty() {
		Err(runtime!("Tried merging nothing into a gas mixture"))
	} else {
		get_mix_mut(src).merge(get_mix(&args[0]));
		Ok(Value::null())
	}
}

#[hook("/datum/gas_mixture/proc/__remove_ratio")]
fn _remove_ratio_hook() {
	if args.len() < 2 {
		Err(runtime!("remove_ratio called with fewer than 2 arguments"))
	} else {
		get_mix_mut(&args[0])
			.copy_from_mutable(&get_mix_mut(src).remove_ratio(args[1].as_number().unwrap_or(0.0)));
		Ok(Value::null())
	}
}

#[hook("/datum/gas_mixture/proc/__remove")]
fn _remove_hook() {
	if args.len() < 2 {
		Err(runtime!("remove called with fewer than 2 arguments"))
	} else {
		get_mix_mut(&args[0])
			.copy_from_mutable(&get_mix_mut(src).remove(args[1].as_number().unwrap_or(0.0)));
		Ok(Value::null())
	}
}

#[hook("/datum/gas_mixture/proc/copy_from")]
fn _copy_from_hook() {
	if args.is_empty() {
		Err(runtime!("Tried copying a gas mix from nothing"))
	} else {
		get_mix_mut(src).copy_from_mutable(get_mix(&args[0]));
		Ok(Value::null())
	}
}

#[hook("/datum/gas_mixture/proc/temperature_share")]
fn _temperature_share_hook() {
	let arg_num = args.len();
	match arg_num {
		2 => Ok(Value::from(get_mix_mut(src).temperature_share(
			get_mix_mut(&args[0]),
			args[1].as_number().unwrap_or(0.0),
		))),
		4 => Ok(Value::from(get_mix_mut(src).temperature_share_non_gas(
			args[0].as_number().unwrap_or(0.0),
			args[1].as_number().unwrap_or(0.0),
			args[2].as_number().unwrap_or(0.0),
		))),
		_ => Err(runtime!("Invalid args for temperature_share")),
	}
}

#[hook("/datum/gas_mixture/proc/get_gases")]
fn _get_gases_hook() {
	let mut gases_list: List = List::new();
	let mix = get_mix(src);
	for gas in mix.get_gases() {
		gases_list.append(*gas as f32);
	}
	Ok(Value::from(gases_list))
}

#[hook("/datum/gas_mixture/proc/set_temperature")]
fn _set_temperature_hook() {
	let v = if args.is_empty() {
		0.0
	} else {
		args[0].as_number().unwrap_or(0.0)
	};
	if !v.is_finite() {
		Err(runtime!(
			"Attempted to set a temperature to a number that is NaN or infinite."
		))
	} else {
		get_mix_mut(src).set_temperature(v.max(2.7));
		Ok(Value::null())
	}
}

#[hook("/datum/gas_mixture/proc/set_volume")]
fn _set_volume_hook() {
	if args.is_empty() {
		Err(runtime!("Attempted to set volume to nothing."))
	} else {
		get_mix_mut(src).volume = args[0].as_number().unwrap_or(0.0);
		Ok(Value::null())
	}
}

#[hook("/datum/gas_mixture/proc/get_moles")]
fn _get_moles_hook() {
	if args.is_empty() {
		Err(runtime!("Incorrect arg len for get_moles (0)."))
	} else {
		let res = gas_id_from_type(&args[0]);
		match res {
			Ok(idx) => Ok(Value::from(get_mix(src).get_moles(idx))),
			Err(e) => Err(e),
		}
	}
}

#[hook("/datum/gas_mixture/proc/set_moles")]
fn _set_moles_hook() {
	if args.len() < 2 {
		return Err(runtime!("Incorrect arg len for set_moles (less than 2)."));
	}
	let vf = args[1].as_number().unwrap_or(0.0);
	if !vf.is_finite() {
		return Err(runtime!("Attempted to set moles to NaN or infinity."));
	}
	if vf < 0.0 {
		return Err(runtime!("Attempted to set moles to a negative number."));
	}
	let res = gas_id_from_type(&args[0]);
	if res.is_ok() {
		let idx = res.unwrap();
		get_mix_mut(src).set_moles(idx, vf);
		Ok(Value::null())
	} else {
		Err(res.err().unwrap())
	}
}

#[hook("/datum/gas_mixture/proc/scrub_into")]
fn _scrub_into_hook() {
	if args.len() < 2 {
		Err(runtime!("Incorrect arg len for scrub_into (less than 2)."))
	} else {
		let src_gas = get_mix_mut(src);
		let dest_gas = get_mix_mut(&args[0]);
		let gases_to_scrub = args[1].as_list().unwrap();
		let mut buffer = gas::gas_mixture::GasMixture::from_vol(gas::constants::CELL_VOLUME);
		buffer.set_temperature(src_gas.get_temperature());
		for idx in 0..gases_to_scrub.len() {
			let res = gas_id_from_type(&gases_to_scrub.get(idx).unwrap());
			if res.is_ok() {
				// it's allowed to continue after failure here
				let idx = res.unwrap();
				buffer.set_moles(idx, buffer.get_moles(idx) + src_gas.get_moles(idx));
				src_gas.set_moles(idx, 0.0);
			}
		}
		dest_gas.merge(&buffer);
		Ok(args[0].clone())
	}
}

#[hook("/datum/gas_mixture/proc/mark_immutable")]
fn _mark_immutable_hook() {
	get_mix_mut(src).mark_immutable();
	Ok(Value::null())
}

#[hook("/datum/gas_mixture/proc/clear")]
fn _clear_hook() {
	get_mix_mut(src).clear();
	Ok(Value::null())
}

#[hook("/datum/gas_mixture/proc/compare")]
fn _compare_hook() {
	if args.is_empty() {
		Err(runtime!("Tried comparing a gas mix to nothing"))
	} else {
		let res = get_mix(src).compare(get_mix(&args[0]));
		match res {
			-1 => Ok(Value::from_string("temp")),
			-2 => Ok(Value::from_string("")),
			_ => gas_id_to_type(res as usize),
		}
	}
}

#[hook("/datum/gas_mixture/proc/multiply")]
fn _multiply_hook() {
	get_mix_mut(src).multiply(if args.is_empty() {
		1.0
	} else {
		args[0].as_number().unwrap_or(1.0)
	});
	Ok(Value::null())
}
