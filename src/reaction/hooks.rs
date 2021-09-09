use auxtools::*;

use crate::gas::{
	constants::*, gas_fusion_power, gas_idx_from_string, with_gas_info, with_mix, with_mix_mut,
	GasIDX,
};

#[cfg(feature = "plasma_fire_hook")]
#[hook("/datum/gas_reaction/plasmafire/react")]
fn _plasma_fire(byond_air: &Value, holder: &Value) {
	const PLASMA_UPPER_TEMPERATURE: f32 = 1390.0 + T0C;
	const OXYGEN_BURN_RATE_BASE: f32 = 1.4;
	const SUPER_SATURATION_THRESHOLD: f32 = 96.0;
	const PLASMA_OXYGEN_FULLBURN: f32 = 10.0;
	const PLASMA_BURN_RATE_DELTA: f32 = 9.0;
	const FIRE_PLASMA_ENERGY_RELEASED: f32 = 3000000.0;
	let o2 = gas_idx_from_string(GAS_O2)?;
	let plasma = gas_idx_from_string(GAS_PLASMA)?;
	let co2 = gas_idx_from_string(GAS_CO2)?;
	let tritium = gas_idx_from_string(GAS_TRITIUM)?;
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
					air.thermal_energy(),
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
				air.adjust_moles(tritium, plasma_burn_rate);
			} else {
				air.adjust_moles(co2, plasma_burn_rate);
			}
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
			if let Some(fire_expose) = Proc::find(byond_string!("/proc/fire_expose")) {
				fire_expose.call(&[holder, byond_air, &Value::from(temperature)])?;
			} else {
				Proc::find(byond_string!("/proc/stack_trace"))
					.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
					.call(&[&Value::from_string(
						"fire_expose not found! Auxmos hooked fires do not work without it!",
					)?])?;
			}
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
	let o2 = gas_idx_from_string(GAS_O2)?;
	let tritium = gas_idx_from_string(GAS_TRITIUM)?;
	let water = gas_idx_from_string(GAS_H2O)?;
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
				// yes, we set burned_fuel to trit times ten. times ten!! and then the actual amount burned is 1% of that.
				// this is why trit bombs are Like That.
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
		cached_results.set(byond_string!("fire"), Value::from(burned_fuel))?;
		air.set_temperature(new_temp);
		air.garbage_collect();
		Ok((burned_fuel, energy_released, new_temp))
	})?;
	if burned_fuel > TRITIUM_MINIMUM_RADIATION_FACTOR {
		if let Some(radiation_burn) = Proc::find(byond_string!("/proc/radiation_burn")) {
			radiation_burn.call(&[holder, &Value::from(energy_released)])?;
		} else {
			let _ = Proc::find(byond_string!("/proc/stack_trace"))
			.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
			.call(&[&Value::from_string(
				"radiation_burn not found! Auxmos hooked trit fires won't irradiated without it!"
			)?]);
		}
	}
	if temperature > FIRE_MINIMUM_TEMPERATURE_TO_EXIST {
		if let Some(fire_expose) = Proc::find(byond_string!("/proc/fire_expose")) {
			fire_expose.call(&[holder, byond_air, &Value::from(temperature)])?;
		} else {
			Proc::find(byond_string!("/proc/stack_trace"))
				.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
				.call(&[&Value::from_string(
					"fire_expose not found! Auxmos hooked fires do not work without it!",
				)?])?;
		}
	}
	Ok(Value::from(1.0))
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
	let plas = gas_idx_from_string(GAS_PLASMA)?;
	let co2 = gas_idx_from_string(GAS_CO2)?;
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
					.fold(0.0, |acc, (i, amt)| acc + gas_fusion_power(&i) * amt),
			))
		})?;
	let instability = (gas_power * INSTABILITY_GAS_FACTOR)
		.powi(2)
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
		let tritium = gas_idx_from_string(GAS_TRITIUM)?;
		let (byproduct_a, byproduct_b, tritium_conversion_coefficient) = {
			if reaction_energy > 0.0 {
				(
					gas_idx_from_string(GAS_O2)?,
					gas_idx_from_string(GAS_NITROUS)?,
					FUSION_TRITIUM_CONVERSION_COEFFICIENT,
				)
			} else {
				(
					gas_idx_from_string(GAS_BZ)?,
					gas_idx_from_string(GAS_NITRYL)?,
					-FUSION_TRITIUM_CONVERSION_COEFFICIENT,
				)
			}
		};
		with_mix_mut(byond_air, |air| {
			air.adjust_moles(tritium, -FUSION_TRITIUM_MOLES_USED);
			air.set_moles(plas, plasma);
			air.set_moles(co2, carbon);
			air.adjust_moles(
				byproduct_a,
				FUSION_TRITIUM_MOLES_USED * (reaction_energy * tritium_conversion_coefficient),
			);
			air.adjust_moles(
				byproduct_b,
				FUSION_TRITIUM_MOLES_USED * (reaction_energy * tritium_conversion_coefficient),
			);
			if reaction_energy != 0.0 {
				air.set_temperature((initial_energy + reaction_energy) / air.heat_capacity());
			}
			air.garbage_collect();
			Ok(())
		})?;
		if reaction_energy != 0.0 {
			if let Some(fusion_ball) = Proc::find(byond_string!("/proc/fusion_ball")) {
				fusion_ball.call(&[
					holder,
					&Value::from(reaction_energy),
					&Value::from(instability),
				])?;
			} else {
				Proc::find(byond_string!("/proc/stack_trace"))
					.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
					.call(&[&Value::from_string(
						"fusion_ball not found! Auxmos hooked fusion does not work without it!",
					)?])?;
			}
			Ok(Value::from(1.0))
		} else {
			Ok(Value::from(0.0))
		}
	}
}

#[cfg(feature = "generic_fire_hook")]
#[hook("/datum/gas_reaction/genericfire/react")]
fn _hook_generic_fire(byond_air: Value, holder: Value) {
	use fxhash::FxBuildHasher;
	use std::collections::HashMap;
	let mut burn_results: HashMap<GasIDX, f32, FxBuildHasher> = HashMap::with_capacity_and_hasher(
		super::total_num_gases() as usize,
		FxBuildHasher::default(),
	);
	let mut energy_released = 0.0;
	with_gas_info(|gas_info| {
		if let Some(fire_amount) = with_mix(&byond_air, |air| {
			let (mut fuels, mut oxidizers) = air.get_fire_info_with_lock(gas_info);
			let oxidation_power = oxidizers
				.iter()
				.copied()
				.fold(0.0, |acc, (_, _, power)| acc + power);
			let total_fuel = fuels
				.iter()
				.copied()
				.fold(0.0, |acc, (_, _, power)| acc + power);
			if oxidation_power < GAS_MIN_MOLES {
				Err(runtime!(
					"Gas has no oxidizer even though it passed oxidizer check!"
				))
			} else if total_fuel <= GAS_MIN_MOLES {
				Err(runtime!(
					"Gas has no fuel even though it passed fuel check!"
				))
			} else {
				let oxidation_ratio = oxidation_power / total_fuel;
				if oxidation_ratio > 1.0 {
					for (_, amt, power) in oxidizers.iter_mut() {
						*amt /= oxidation_ratio;
						*power /= oxidation_ratio;
					}
				} else {
					for (_, amt, power) in fuels.iter_mut() {
						*amt *= oxidation_ratio;
						*power *= oxidation_ratio;
					}
				}
				for (i, a, p) in oxidizers.iter().copied().chain(fuels.iter().copied()) {
					let amt = FIRE_MAXIMUM_BURN_RATE * a;
					let power = FIRE_MAXIMUM_BURN_RATE * p;
					let this_gas_info = &gas_info[i as usize];
					energy_released += power * this_gas_info.fire_energy_released;
					if let Some(products) = this_gas_info.fire_products.as_ref() {
						for (product_idx, product_amt) in products.iter() {
							burn_results
								.entry(product_idx.get()?)
								.and_modify(|r| *r += product_amt * amt)
								.or_insert_with(|| product_amt * amt);
						}
					}
					burn_results
						.entry(i)
						.and_modify(|r| *r -= amt)
						.or_insert(-amt);
				}
				Ok(Some(
					oxidation_power.min(total_fuel) * 2.0 * FIRE_MAXIMUM_BURN_RATE,
				))
			}
		})? {
			let temperature = with_mix_mut(&byond_air, |air| {
				let final_energy = air.thermal_energy() + energy_released;
				for (&i, &amt) in burn_results.iter() {
					air.adjust_moles(i, amt);
				}
				air.set_temperature(final_energy / air.heat_capacity());
				Ok(air.get_temperature())
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
				if let Some(fire_expose) = Proc::find(byond_string!("/proc/fire_expose")) {
					fire_expose.call(&[holder, byond_air, &Value::from(temperature)])?;
				} else {
					Proc::find(byond_string!("/proc/stack_trace"))
						.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
						.call(&[&Value::from_string(
							"fire_expose not found! Auxmos hooked fires do not work without it!",
						)?])?;
				}
			}
			Ok(Value::from(if fire_amount > 0.0 { 1.0 } else { 0.0 }))
		} else {
			Ok(Value::from(0.0))
		}
	})
}
