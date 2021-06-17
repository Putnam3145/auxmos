use super::{constants::*, gas_mixture::*, with_mix, with_mix_mut, GasIDX};

use auxtools::*;

use crate::gas_idx_from_string;

#[cfg(feature = "plasma_fire_hook")]
static mut PLAS_FIRE_GASES: Option<(GasIDX, GasIDX, GasIDX, GasIDX)> = None;

#[cfg(feature = "trit_fire_hook")]
static mut TRIT_FIRE_GASES: Option<(GasIDX, GasIDX, GasIDX)> = None;

#[cfg(feature = "fusion_hook")]
static mut FUSION_GASES: Option<(GasIDX, GasIDX, GasIDX, GasIDX, GasIDX, GasIDX, GasIDX)> = None;

#[shutdown]
fn _shutdown_reaction_hooks() {
	#[cfg(feature = "plasma_fire_hook")]
	unsafe {
		PLAS_FIRE_GASES = None
	}
	#[cfg(feature = "trit_fire_hook")]
	unsafe {
		TRIT_FIRE_GASES = None
	}
	#[cfg(feature = "fusion_hook")]
	unsafe {
		FUSION_GASES = None
	}
}

#[cfg(feature = "plasma_fire_hook")]
#[hook("/datum/gas_reaction/plasmafire/react")]
fn _plasma_fire(byond_air: &Value, holder: &Value) {
	const PLASMA_UPPER_TEMPERATURE: f32 = 1390.0 + super::constants::T0C;
	const OXYGEN_BURN_RATE_BASE: f32 = 1.4;
	const SUPER_SATURATION_THRESHOLD: f32 = 96.0;
	const PLASMA_OXYGEN_FULLBURN: f32 = 10.0;
	const PLASMA_BURN_RATE_DELTA: f32 = 9.0;
	const FIRE_PLASMA_ENERGY_RELEASED: f32 = 3000000.0;
	let (o2, plasma, co2, tritium) = unsafe {
		if let Some(tup) = PLAS_FIRE_GASES {
			tup
		} else {
			let tup = (
				gas_idx_from_string(GAS_O2)?,
				gas_idx_from_string(GAS_PLASMA)?,
				gas_idx_from_string(GAS_CO2)?,
				gas_idx_from_string(GAS_TRITIUM)?,
			);
			PLAS_FIRE_GASES = Some(tup);
			tup
		}
	};
	let (oxygen_burn_rate, plasma_burn_rate, initial_oxy, initial_plasma, initial_energy) =
		with_mix(&byond_air, |air| {
			let temperature_scale = {
				if air.get_temperature() > PLASMA_UPPER_TEMPERATURE {
					1.0
				} else {
					(air.get_temperature() - FIRE_MINIMUM_TEMPERATURE_TO_EXIST)
						/ (PLASMA_UPPER_TEMPERATURE - FIRE_MINIMUM_TEMPERATURE_TO_EXIST)
				}
			};
			if temperature_scale > 0.0 {
				let oxygen_burn_rate = OXYGEN_BURN_RATE_BASE - temperature_scale;
				let oxy = air.get_moles(o2);
				let plas = air.get_moles(plasma);
				let plasma_burn_rate = {
					if oxy > plas * PLASMA_OXYGEN_FULLBURN {
						plas * temperature_scale / PLASMA_BURN_RATE_DELTA
					} else {
						(temperature_scale * (oxy / PLASMA_OXYGEN_FULLBURN))
							/ PLASMA_BURN_RATE_DELTA
					}
				}
				.min(plas)
				.min(oxy / oxygen_burn_rate);
				Ok((
					oxygen_burn_rate,
					plasma_burn_rate,
					oxy,
					plas,
					air.heat_capacity() * air.get_temperature(),
				))
			} else {
				Ok((0.0, -1.0, 0.0, 0.0, 0.0))
			}
		})?;
	let fire_amount = plasma_burn_rate * (1.0 + oxygen_burn_rate);
	if fire_amount > 0.0 {
		let gas_changes = [
			(plasma, -plasma_burn_rate),
			(o2, -plasma_burn_rate * oxygen_burn_rate),
			(
				if initial_oxy / initial_plasma > SUPER_SATURATION_THRESHOLD {
					tritium
				} else {
					co2
				},
				plasma_burn_rate,
			),
		];
		let temperature = with_mix_mut(&byond_air, |air| {
			air.adjust_multiple(&gas_changes);
			let new_temp = (initial_energy + plasma_burn_rate * FIRE_PLASMA_ENERGY_RELEASED)
				/ air.heat_capacity();
			air.set_temperature(new_temp);
			air.garbage_collect();
			Ok(new_temp)
		})?;
		let cached_results = byond_air
			.get_list(byond_string!("reaction_results"))
			.map_err(|_| {
				runtime!(
					"Attempt to interpret non-list value as list {} {}:{}",
					std::file!(),
					std::line!(),
					std::column!()
				)
			})?;
		cached_results.set(byond_string!("fire"), Value::from(fire_amount))?;
		if temperature > FIRE_MINIMUM_TEMPERATURE_TO_EXIST {
			Proc::find(byond_string!("/proc/fire_expose"))
				.unwrap()
				.call(&[holder, byond_air, &Value::from(temperature)])?;
		}
		Ok(Value::from(1.0))
	} else {
		Ok(Value::from(0.0))
	}
}

#[cfg(feature = "trit_fire_hook")]
#[hook("/datum/gas_reaction/tritfire/react")]
fn tritfire(byond_air: &Value, holder: &Value) {
	const TRITIUM_BURN_OXY_FACTOR: f32 = 100.0;
	const TRITIUM_BURN_TRIT_FACTOR: f32 = 10.0;
	const TRITIUM_MINIMUM_RADIATION_FACTOR: f32 = 0.1;
	const FIRE_HYDROGEN_ENERGY_RELEASED: f32 = 280000.0;
	let (o2, tritium, water) = unsafe {
		if let Some(tup) = TRIT_FIRE_GASES {
			tup
		} else {
			let tup = (
				gas_idx_from_string(GAS_O2)?,
				gas_idx_from_string(GAS_TRITIUM)?,
				gas_idx_from_string(GAS_H2O)?,
			);
			TRIT_FIRE_GASES = Some(tup);
			tup
		}
	};
	let (burned_fuel, energy_released, temperature) = with_mix_mut(byond_air, |air| {
		let initial_oxy = air.get_moles(o2);
		let initial_trit = air.get_moles(tritium);
		let initial_energy = air.thermal_energy();
		let burned_fuel = {
			if initial_oxy < initial_trit {
				let r = initial_oxy / TRITIUM_BURN_OXY_FACTOR;
				air.set_moles(tritium, initial_trit - r);
				r
			} else {
				let r = initial_trit * TRITIUM_BURN_TRIT_FACTOR;
				air.set_moles(
					tritium,
					initial_trit - initial_trit / TRITIUM_BURN_TRIT_FACTOR,
				);
				air.set_moles(o2, initial_oxy - initial_trit);
				r
			}
		};
		air.adjust_moles(water, burned_fuel / TRITIUM_BURN_OXY_FACTOR);
		let energy_released = FIRE_HYDROGEN_ENERGY_RELEASED * burned_fuel;
		let new_temp = (initial_energy + energy_released) / air.heat_capacity();
		air.set_temperature(new_temp);
		air.garbage_collect();
		Ok((burned_fuel, energy_released, new_temp))
	})?;
	if burned_fuel > TRITIUM_MINIMUM_RADIATION_FACTOR {
		Proc::find(byond_string!("/proc/radiation_burn"))
			.unwrap()
			.call(&[holder, &Value::from(energy_released)])?;
	}
	if temperature > FIRE_MINIMUM_TEMPERATURE_TO_EXIST {
		Proc::find(byond_string!("/proc/fire_expose"))
			.unwrap()
			.call(&[holder, byond_air, &Value::from(temperature)])?;
	}
	Ok(Value::from(burned_fuel > 0.0))
}

#[cfg(feature = "fusion_hook")]
#[hook("/datum/gas_reaction/fusion/react")]
fn fusion(byond_air: Value, holder: Value) {
	use std::f32::consts::PI;
	const TOROID_VOLUME_BREAKEVEN: f32 = 1000.0;
	const INSTABILITY_GAS_FACTOR: f32 = 0.003;
	const PLASMA_BINDING_ENERGY: f32 = 20_000_000.0;
	const FUSION_TRITIUM_MOLES_USED: f32 = 1.0;
	const FUSION_INSTABILITY_ENDOTHERMALITY: f32 = 2.0;
	const FUSION_TRITIUM_CONVERSION_COEFFICIENT: f32 = 1E-10;
	const FUSION_MOLE_THRESHOLD: f32 = 250.0;
	let (tritium, plas, co2, o2, nitrous, bz, nitryl) = unsafe {
		if let Some(tup) = FUSION_GASES {
			tup
		} else {
			let tup = (
				gas_idx_from_string(GAS_TRITIUM)?,
				gas_idx_from_string(GAS_PLASMA)?,
				gas_idx_from_string(GAS_CO2)?,
				gas_idx_from_string(GAS_O2)?,
				gas_idx_from_string(GAS_NITROUS)?,
				gas_idx_from_string(GAS_BZ)?,
				gas_idx_from_string(GAS_NITRYL)?,
			);
			FUSION_GASES = Some(tup);
			tup
		}
	};
	let (initial_energy, initial_plasma, initial_carbon, scale_factor, toroidal_size, gas_power) =
		with_mix(byond_air, |air| {
			Ok((
				air.thermal_energy(),
				air.get_moles(plas),
				air.get_moles(co2),
				air.volume / PI,
				(2.0 * PI)
					+ ((air.volume - TOROID_VOLUME_BREAKEVEN) / TOROID_VOLUME_BREAKEVEN).atan(),
				air.gases()
					.dot_product(crate::gases::FUSION_POWERS.read().as_ref().unwrap()),
			))
		})?;
	let instability = (gas_power * INSTABILITY_GAS_FACTOR)
		.powf(2.0)
		.rem_euclid(toroidal_size);
	byond_air.call("set_analyzer_results", &[&Value::from(instability)])?;
	let mut plasma = (initial_plasma - FUSION_MOLE_THRESHOLD) / scale_factor;
	let mut carbon = (initial_carbon - FUSION_MOLE_THRESHOLD) / scale_factor;
	plasma = (plasma - instability * carbon.sin()).rem_euclid(toroidal_size);
	carbon = (carbon - plasma).rem_euclid(toroidal_size);
	plasma = plasma * scale_factor + FUSION_MOLE_THRESHOLD;
	carbon = carbon * scale_factor + FUSION_MOLE_THRESHOLD;
	let reaction_energy = {
		let delta_plasma = initial_plasma - plasma;
		if delta_plasma < 0.0 {
			if instability < FUSION_INSTABILITY_ENDOTHERMALITY {
				0.0
			} else {
				delta_plasma
					* PLASMA_BINDING_ENERGY
					* (instability - FUSION_INSTABILITY_ENDOTHERMALITY).sqrt()
			}
		} else {
			delta_plasma * PLASMA_BINDING_ENERGY
		}
	};
	if initial_energy + reaction_energy < 0.0 {
		Ok(Value::from(0.0))
	} else {
		let (byproduct_a, byproduct_b, tritium_conversion_amount) = {
			if reaction_energy > 0.0 {
				(
					o2,
					nitrous,
					FUSION_TRITIUM_MOLES_USED
						* reaction_energy * FUSION_TRITIUM_CONVERSION_COEFFICIENT,
				)
			} else {
				(
					bz,
					nitryl,
					-FUSION_TRITIUM_MOLES_USED
						* reaction_energy * FUSION_TRITIUM_CONVERSION_COEFFICIENT,
				)
			}
		};
		let gas_changes = [
			(tritium, -FUSION_TRITIUM_MOLES_USED),
			(byproduct_a, tritium_conversion_amount),
			(byproduct_b, tritium_conversion_amount),
		];
		if with_mix_mut(byond_air, |air| {
			air.set_moles(plas, plasma);
			air.set_moles(co2, carbon);
			air.adjust_multiple(&gas_changes);
			air.garbage_collect();
			if reaction_energy.is_normal() {
				air.set_temperature((initial_energy + reaction_energy) / air.heat_capacity());
				Ok(true)
			} else {
				Ok(false)
			}
		})? {
			Proc::find(byond_string!("/proc/fusion_ball"))
				.unwrap()
				.call(&[
					holder,
					&Value::from(reaction_energy),
					&Value::from(instability),
				])?;
			Ok(Value::from(REACTING))
		} else {
			Ok(Value::from(NO_REACTION))
		}
	}
}

#[cfg(feature = "generic_fire_hook")]
#[hook("/datum/gas_reaction/genericfire/react")]
fn _hook_generic_fire(byond_air: Value, holder: Value) {
	let mut burn_results = FlatSimdVec::new();
	let mut energy_released = 0.0;
	with_mix(&byond_air, |air| {
		use super::{
			with_fire_products, FIRE_ENTHALPIES, FIRE_RATES, FIRE_TEMPS, OXIDATION_POWERS,
			OXIDATION_TEMPS,
		};
		let temp_vec = SimdGasVec::splat(air.get_temperature());
		let mut oxidizers: Vec<(_, _)> = air
			.gases()
			.iter()
			.copied()
			.zip(
				OXIDATION_TEMPS
					.read()
					.as_ref()
					.unwrap()
					.iter()
					.copied()
					.zip(OXIDATION_POWERS.read().as_ref().unwrap().iter().copied()),
			)
			.map(|(mut amt, (oxi_temp, oxi_pwr))| {
				amt = temp_vec.ge(oxi_temp).select(amt, SimdGasVec::splat(0.0));
				let mut powers = amt.clone();
				powers *= oxi_pwr;
				let temperature_scale = 1.0 - (oxi_temp / air.get_temperature());
				powers *= temperature_scale;
				(amt, powers)
			})
			.collect();
		let mut fuels: Vec<(_, _)> = air
			.gases()
			.iter()
			.copied()
			.zip(
				FIRE_TEMPS
					.read()
					.as_ref()
					.unwrap()
					.iter()
					.copied()
					.zip(FIRE_RATES.read().as_ref().unwrap().iter().copied()),
			)
			.map(|(mut amt, (fire_temp, fire_rate))| {
				amt = temp_vec.ge(fire_temp).select(amt, SimdGasVec::splat(0.0));
				let mut powers = amt.clone();
				powers /= fire_rate;
				let temperature_scale = 1.0 - (fire_temp / air.get_temperature());
				powers *= temperature_scale;
				(amt, powers)
			})
			.collect();
		let oxidation_power = oxidizers
			.iter()
			.copied()
			.fold(0.0f32, |acc, (_, power)| acc + power.sum());
		let total_fuel = fuels
			.iter()
			.copied()
			.fold(0.0f32, |acc, (_, power)| acc + power.sum());
		if oxidation_power <= 0.0 || total_fuel <= 0.0 {
			Ok(None)
		} else {
			let oxidation_ratio = oxidation_power / total_fuel;
			if oxidation_ratio > 1.0 {
				for (amt, power) in oxidizers.iter_mut() {
					*amt /= oxidation_ratio;
					*power /= oxidation_ratio;
				}
			} else {
				for (amt, power) in fuels.iter_mut() {
					*amt *= oxidation_ratio;
					*power *= oxidation_ratio;
				}
			}
			let all_amounts = FlatSimdVec::from_vec(
				oxidizers
					.iter()
					.copied()
					.zip(fuels.iter().copied())
					.map(|(a, b)| a.0 + b.0)
					.collect(),
			);
			let all_powers = FlatSimdVec::from_vec(
				oxidizers
					.iter()
					.copied()
					.zip(fuels.iter().copied())
					.map(|(a, b)| a.1 + b.1)
					.collect(),
			);
			with_fire_products(|fire_products| {
				for (i, product_vec) in fire_products.iter().enumerate() {
					burn_results += product_vec * all_powers.get(i).unwrap_or_default();
				}
			});
			{
				energy_released = (all_amounts * FIRE_ENTHALPIES.read().as_ref().unwrap()).sum();
			}
			Ok(Some(oxidation_power.min(total_fuel) * 2.0))
		}
	})
	.and_then(|maybe_fire_amount| {
		if let Some(fire_amount) = maybe_fire_amount {
			let temperature = with_mix_mut(&byond_air, |air| {
				air.merge_from_vec_with_energy(&burn_results, energy_released);
				Ok(air.get_temperature())
			})?;
			byond_air
				.get_list(byond_string!("reaction_results"))
				.map_err(|_| {
					runtime!(
						"Attempt to interpret non-list value as list {} {}:{}",
						std::file!(),
						std::line!(),
						std::column!()
					)
				})?
				.set(byond_string!("fire"), Value::from(fire_amount))?;
			if temperature > FIRE_MINIMUM_TEMPERATURE_TO_EXIST {
				// not a guarantee by any means!
				Proc::find(byond_string!("/proc/fire_expose"))
					.ok_or_else(|| runtime!("Could not find fire_expose"))?
					.call(&[holder, byond_air, &Value::from(temperature)])?;
			}
			Ok(if fire_amount > 0.0 {
				REACTING
			} else {
				NO_REACTION
			})
		} else {
			Ok(NO_REACTION)
		}
	})
	.map(|r| Value::from(r))
}
