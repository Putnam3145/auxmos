use super::{with_mix, with_mix_mut};

use super::constants::*;

use auxtools::*;

use crate::{gas_fusion_power, gas_id_from_type_name};

#[feature("plasma_fire_hook")]
#[hook("/datum/gas_reaction/plasmafire/react")]
fn _plasma_fire(byond_air: &Value, holder: &Value) {
	const PLASMA_UPPER_TEMPERATURE: f32 = 1390.0 + super::constants::T0C;
	const OXYGEN_BURN_RATE_BASE: f32 = 1.4;
	const SUPER_SATURATION_THRESHOLD: f32 = 96.0;
	const PLASMA_OXYGEN_FULLBURN: f32 = 10.0;
	const PLASMA_BURN_RATE_DELTA: f32 = 9.0;
	const FIRE_PLASMA_ENERGY_RELEASED: f32 = 3000000.0;
	let o2 = gas_id_from_type_name("oxygen")?;
	let plasma = gas_id_from_type_name("plasma")?;
	let co2 = gas_id_from_type_name("carbon_dioxide")?;
	let tritium = gas_id_from_type_name("tritium")?;
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
		let temperature = with_mix_mut(&byond_air, |air| {
			air.set_moles(plasma, initial_plasma - plasma_burn_rate);
			air.set_moles(o2, initial_oxy - (plasma_burn_rate * oxygen_burn_rate));
			if initial_oxy / initial_plasma > SUPER_SATURATION_THRESHOLD {
				air.set_moles(tritium, air.get_moles(tritium) + plasma_burn_rate);
			} else {
				air.set_moles(co2, air.get_moles(co2) + plasma_burn_rate);
			}
			let new_temp = (initial_energy + plasma_burn_rate * FIRE_PLASMA_ENERGY_RELEASED)
				/ air.heat_capacity();
			air.set_temperature(new_temp);
			air.garbage_collect();
			Ok(new_temp)
		})?;
		let cached_results = byond_air.get_list(byond_string!("reaction_results"))?;
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

#[feature("trit_fire_hook")]
#[hook("/datum/gas_reaction/tritfire/react")]
fn tritfire(byond_air: &Value, holder: &Value) {
	const TRITIUM_BURN_OXY_FACTOR: f32 = 100.0;
	const TRITIUM_BURN_TRIT_FACTOR: f32 = 10.0;
	const TRITIUM_MINIMUM_RADIATION_FACTOR: f32 = 0.1;
	const FIRE_HYDROGEN_ENERGY_RELEASED: f32 = 280000.0;
	let o2 = gas_id_from_type_name("oxygen")?;
	let tritium = gas_id_from_type_name("tritium")?;
	let water = gas_id_from_type_name("water_vapor")?;
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
		air.set_moles(
			water,
			air.get_moles(water) + burned_fuel / TRITIUM_BURN_OXY_FACTOR,
		);
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
	Ok(Value::from(1.0))
}

#[feature("fusion_hook")]
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
	let plas = gas_id_from_type_name("plasma")?;
	let co2 = gas_id_from_type_name("carbon_dioxide")?;
	let (initial_energy, initial_plasma, initial_carbon, scale_factor, toroidal_size, gas_power) =
		with_mix(byond_air, |air| {
			Ok((
				air.thermal_energy(),
				air.get_moles(plas),
				air.get_moles(co2),
				air.volume / PI,
				(2.0 * PI)
					+ ((air.volume - TOROID_VOLUME_BREAKEVEN) / TOROID_VOLUME_BREAKEVEN).atan(),
				air.enumerate()
					.fold(0.0, |acc, (i, amt)| acc + gas_fusion_power(i) * amt),
			))
		})?;
	let instability = (gas_power * INSTABILITY_GAS_FACTOR)
		.powf(2.0)
		.rem_euclid(toroidal_size);
	byond_air.call("set_analyzer_results", &[&Value::from(instability)])?;
	let mut plasma = (initial_plasma - FUSION_MOLE_THRESHOLD) / scale_factor;
	let mut carbon = (initial_carbon - FUSION_MOLE_THRESHOLD) / scale_factor;
	plasma = (plasma - instability * carbon.sin()).rem_euclid(toroidal_size);
	//count the rings. ss13's modulus is positive, this ain't, who knew
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
		let tritium = gas_id_from_type_name("tritium")?;
		let (byproduct_a, byproduct_b, tritium_conversion_coefficient) = {
			if reaction_energy > 0.0 {
				(
					gas_id_from_type_name("oxygen")?,
					gas_id_from_type_name("nitrous_oxide")?,
					FUSION_TRITIUM_CONVERSION_COEFFICIENT,
				)
			} else {
				(
					gas_id_from_type_name("bz")?,
					gas_id_from_type_name("nitryl")?,
					-FUSION_TRITIUM_CONVERSION_COEFFICIENT,
				)
			}
		};
		with_mix_mut(byond_air, |air| {
			air.set_moles(tritium, air.get_moles(tritium) - FUSION_TRITIUM_MOLES_USED);
			air.set_moles(plas, plasma);
			air.set_moles(co2, carbon);
			air.set_moles(
				byproduct_a,
				air.get_moles(byproduct_a)
					+ FUSION_TRITIUM_MOLES_USED
						* (reaction_energy * tritium_conversion_coefficient),
			);
			air.set_moles(
				byproduct_b,
				air.get_moles(byproduct_b)
					+ FUSION_TRITIUM_MOLES_USED
						* (reaction_energy * tritium_conversion_coefficient),
			);
			if reaction_energy != 0.0 {
				air.set_temperature((initial_energy + reaction_energy) / air.heat_capacity());
			}
			air.garbage_collect();
			Ok(())
		})?;
		if reaction_energy != 0.0 {
			Proc::find("/proc/fusion_ball").unwrap().call(&[
				holder,
				&Value::from(reaction_energy),
				&Value::from(instability),
			])?;
			Ok(Value::from(1.0))
		} else {
			Ok(Value::from(0.0))
		}
	}
}
