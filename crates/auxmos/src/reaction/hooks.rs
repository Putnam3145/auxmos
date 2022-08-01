use auxtools::*;

use crate::gas::{
	constants::*, gas_fusion_power, gas_idx_from_string, with_gas_info, with_mix, with_mix_mut,
	FireProductInfo, GasIDX,
};

const SUPER_SATURATION_THRESHOLD: f32 = 96.0;

#[must_use]
pub fn func_from_id(id: &str) -> Option<ReactFunc> {
	match id {
		#[cfg(feature = "plasma_fire_hook")]
		"plasmafire" => Some(plasma_fire),
		#[cfg(feature = "trit_fire_hook")]
		"tritfire" => Some(tritium_fire),
		#[cfg(feature = "fusion_hook")]
		"fusion" => Some(fusion),
		#[cfg(feature = "generic_fire_hook")]
		"genericfire" => Some(generic_fire),
		_ => None,
	}
}

type ReactFunc = fn(&Value, &Value) -> DMResult<Value>;

#[cfg(feature = "plasma_fire_hook")]
fn plasma_fire(byond_air: &Value, holder: &Value) -> DMResult<Value> {
	const PLASMA_UPPER_TEMPERATURE: f32 = 1390.0 + T0C;
	const OXYGEN_BURN_RATE_BASE: f32 = 1.4;
	const PLASMA_OXYGEN_FULLBURN: f32 = 10.0;
	const PLASMA_BURN_RATE_DELTA: f32 = 9.0;
	const FIRE_PLASMA_ENERGY_RELEASED: f32 = 3_000_000.0;
	let o2 = gas_idx_from_string(GAS_O2)?;
	let plasma = gas_idx_from_string(GAS_PLASMA)?;
	let co2 = gas_idx_from_string(GAS_CO2)?;
	let tritium = gas_idx_from_string(GAS_TRITIUM)?;
	let (oxygen_burn_rate, plasma_burn_rate, initial_oxy, initial_plasma, initial_energy) =
		with_mix(byond_air, |air| {
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
		let temperature = with_mix_mut(byond_air, |air| {
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
fn tritium_fire(byond_air: &Value, holder: &Value) -> DMResult<Value> {
	const TRITIUM_BURN_OXY_FACTOR: f32 = 100.0;
	const TRITIUM_BURN_TRIT_FACTOR: f32 = 10.0;
	const TRITIUM_MINIMUM_RADIATION_FACTOR: f32 = 0.1;
	const FIRE_HYDROGEN_ENERGY_RELEASED: f32 = 280_000.0;
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
			drop(Proc::find(byond_string!("/proc/stack_trace"))
			.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
			.call(&[&Value::from_string(
				"radiation_burn not found! Auxmos hooked trit fires won't irradiated without it!"
			)?]));
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
fn fusion(byond_air: &Value, holder: &Value) -> DMResult<Value> {
	const TOROID_CALCULATED_THRESHOLD: f32 = 5.96; // changing it by 0.1 generally doubles or halves fusion temps
	const INSTABILITY_GAS_POWER_FACTOR: f32 = 3.0;
	const PLASMA_BINDING_ENERGY: f32 = 20_000_000.0;
	const FUSION_TRITIUM_MOLES_USED: f32 = 1.0;
	const FUSION_INSTABILITY_ENDOTHERMALITY: f32 = 2.0;
	const FUSION_TRITIUM_CONVERSION_COEFFICIENT: f32 = 0.002;
	const FUSION_MOLE_THRESHOLD: f32 = 250.0;
	const FUSION_SCALE_DIVISOR: f32 = 10.0; // Used to be Pi
	const FUSION_MINIMAL_SCALE: f32 = 50.0;
	const FUSION_SLOPE_DIVISOR: f32 = 1250.0; // This number is probably the safest number to change
	const FUSION_ENERGY_TRANSLATION_EXPONENT: f32 = 1.25; // This number is probably the most dangerous number to change
	const FUSION_BASE_TEMPSCALE: f32 = 6.0; // This number is responsible for orchestrating fusion temperatures
	const FUSION_MIDDLE_ENERGY_REFERENCE: f32 = 1E+6; // This number is deceptively dangerous; sort of tied to TOROID_CALCULATED_THRESHOLD
	const FUSION_BUFFER_DIVISOR: f32 = 1.0; // Increase this to cull unrobust fusions faster
	const INFINITY: f32 = 1E+30; // Well, infinity in byond
	let plas = gas_idx_from_string(GAS_PLASMA)?;
	let co2 = gas_idx_from_string(GAS_CO2)?;
	let trit = gas_idx_from_string(GAS_TRITIUM)?;
	let h2o = gas_idx_from_string(GAS_H2O)?;
	let bz = gas_idx_from_string(GAS_BZ)?;
	let o2 = gas_idx_from_string(GAS_O2)?;
	let (
		initial_energy,
		initial_plasma,
		initial_carbon,
		scale_factor,
		temperature_scale,
		gas_power,
	) = with_mix(byond_air, |air| {
		Ok((
			air.thermal_energy(),
			air.get_moles(plas),
			air.get_moles(co2),
			(air.volume / FUSION_SCALE_DIVISOR).max(FUSION_MINIMAL_SCALE),
			air.get_temperature().log10(),
			air.enumerate()
				.fold(0.0, |acc, (i, amt)| acc + gas_fusion_power(&i) * amt),
		))
	})?;
	//The size of the phase space hypertorus
	let toroidal_size = TOROID_CALCULATED_THRESHOLD + {
		if temperature_scale <= FUSION_BASE_TEMPSCALE {
			(temperature_scale - FUSION_BASE_TEMPSCALE) / FUSION_BUFFER_DIVISOR
		} else {
			(4.0_f32.powf(temperature_scale - FUSION_BASE_TEMPSCALE)) / FUSION_SLOPE_DIVISOR
		}
	};
	let instability = (gas_power * INSTABILITY_GAS_POWER_FACTOR).rem_euclid(toroidal_size);
	byond_air.call("set_analyzer_results", &[&Value::from(instability)])?;
	let mut thermal_energy = initial_energy;

	//We have to scale the amounts of carbon and plasma down a significant amount in order to show the chaotic dynamics we want
	let mut plasma = (initial_plasma - FUSION_MOLE_THRESHOLD) / scale_factor;
	//We also subtract out the threshold amount to make it harder for fusion to burn itself out.
	let mut carbon = (initial_carbon - FUSION_MOLE_THRESHOLD) / scale_factor;

	//count the rings. ss13's modulus is positive, this ain't, who knew
	plasma = (plasma - instability * carbon.sin()).rem_euclid(toroidal_size);
	carbon = (carbon - plasma).rem_euclid(toroidal_size);

	//Scales the gases back up
	plasma = plasma * scale_factor + FUSION_MOLE_THRESHOLD;
	carbon = carbon * scale_factor + FUSION_MOLE_THRESHOLD;

	let delta_plasma = (initial_plasma - plasma).min(toroidal_size * scale_factor * 1.5);

	//Energy is gained or lost corresponding to the creation or destruction of mass.
	//Low instability prevents endothermality while higher instability acutally encourages it.
	//Reaction energy can be negative or positive, for both exothermic and endothermic reactions.
	let reaction_energy = {
		if (delta_plasma > 0.0) || (instability <= FUSION_INSTABILITY_ENDOTHERMALITY) {
			(delta_plasma * PLASMA_BINDING_ENERGY).max(0.0)
		} else {
			delta_plasma
				* PLASMA_BINDING_ENERGY
				* ((instability - FUSION_INSTABILITY_ENDOTHERMALITY).sqrt())
		}
	};

	//To achieve faster equilibrium. Too bad it is not that good at cooling down.
	if reaction_energy != 0.0 {
		let middle_energy = (((TOROID_CALCULATED_THRESHOLD / 2.0) * scale_factor)
			+ FUSION_MOLE_THRESHOLD)
			* (200.0 * FUSION_MIDDLE_ENERGY_REFERENCE);
		thermal_energy = middle_energy
			* FUSION_ENERGY_TRANSLATION_EXPONENT.powf((thermal_energy / middle_energy).log10());
		//This bowdlerization is a double-edged sword. Tread with care!
		let bowdlerized_reaction_energy = reaction_energy.clamp(
			thermal_energy * ((1.0 / (FUSION_ENERGY_TRANSLATION_EXPONENT.powi(2))) - 1.0),
			thermal_energy * (FUSION_ENERGY_TRANSLATION_EXPONENT.powi(2) - 1.0),
		);
		thermal_energy = middle_energy
			* 10_f32.powf(
				((thermal_energy + bowdlerized_reaction_energy) / middle_energy)
					.log(FUSION_ENERGY_TRANSLATION_EXPONENT),
			);
	};

	//The decay of the tritium and the reaction's energy produces waste gases, different ones depending on whether the reaction is endo or exothermic
	let standard_waste_gas_output =
		scale_factor * (FUSION_TRITIUM_CONVERSION_COEFFICIENT * FUSION_TRITIUM_MOLES_USED);

	let standard_energy = with_mix_mut(byond_air, |air| {
		air.set_moles(plas, plasma);
		air.set_moles(co2, carbon);

		//The reason why you should set up a tritium production line.
		air.adjust_moles(trit, -FUSION_TRITIUM_MOLES_USED);

		//Adds waste products
		if delta_plasma > 0.0 {
			air.adjust_moles(h2o, standard_waste_gas_output);
		} else {
			air.adjust_moles(bz, standard_waste_gas_output);
		}
		air.adjust_moles(o2, standard_waste_gas_output); //Oxygen is a bit touchy subject

		let new_heat_cap = air.heat_capacity();
		let standard_energy = 400_f32 * air.get_moles(plas) * air.get_temperature(); //Prevents putting meaningless waste gases to achieve high rads.

		//Change the temperature
		if new_heat_cap > MINIMUM_HEAT_CAPACITY
			&& (reaction_energy != 0.0 || instability <= FUSION_INSTABILITY_ENDOTHERMALITY)
		{
			air.set_temperature((thermal_energy / new_heat_cap).clamp(TCMB, INFINITY));
		}

		air.garbage_collect();
		Ok(standard_energy)
	})?;
	if reaction_energy != 0.0 {
		Proc::find(byond_string!("/proc/fusion_ball"))
			.unwrap()
			.call(&[
				holder,
				&Value::from(reaction_energy),
				&Value::from(standard_energy),
			])?;
		Ok(Value::from(1.0))
	} else if reaction_energy == 0.0 && instability <= FUSION_INSTABILITY_ENDOTHERMALITY {
		Ok(Value::from(1.0))
	} else {
		Ok(Value::from(0.0))
	}
}

#[cfg(feature = "generic_fire_hook")]
fn generic_fire(byond_air: &Value, holder: &Value) -> DMResult<Value> {
	use fxhash::FxBuildHasher;
	use std::collections::HashMap;
	let mut burn_results: HashMap<GasIDX, f32, FxBuildHasher> = HashMap::with_capacity_and_hasher(
		super::total_num_gases() as usize,
		FxBuildHasher::default(),
	);
	let mut radiation_released = 0.0;
	with_gas_info(|gas_info| {
		if let Some(fire_amount) = with_mix(byond_air, |air| {
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
					for (_, amt, power) in &mut oxidizers {
						*amt /= oxidation_ratio;
						*power /= oxidation_ratio;
					}
				} else {
					for (_, amt, power) in &mut fuels {
						*amt *= oxidation_ratio;
						*power *= oxidation_ratio;
					}
				}
				for (i, a, _) in oxidizers.iter().copied().chain(fuels.iter().copied()) {
					let amt = FIRE_MAXIMUM_BURN_RATE * a;
					let this_gas_info = &gas_info[i as usize];
					radiation_released += amt * this_gas_info.fire_radiation_released;
					if let Some(product_info) = this_gas_info.fire_products.as_ref() {
						match product_info {
							FireProductInfo::Generic(products) => {
								for (product_idx, product_amt) in products.iter() {
									burn_results
										.entry(product_idx.get()?)
										.and_modify(|r| *r += product_amt * amt)
										.or_insert_with(|| product_amt * amt);
								}
							}
							FireProductInfo::Plasma => {
								let product = if oxidation_ratio > SUPER_SATURATION_THRESHOLD {
									GAS_TRITIUM
								} else {
									GAS_CO2
								};
								burn_results
									.entry(gas_idx_from_string(product)?)
									.and_modify(|r| *r += amt)
									.or_insert_with(|| amt);
							}
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
			let temperature = with_mix_mut(byond_air, |air| {
				// internal energy + PV, which happens to be reducible to this
				let initial_enthalpy = air.get_temperature()
					* (air.heat_capacity() + R_IDEAL_GAS_EQUATION * air.total_moles());
				let mut delta_enthalpy = 0.0;
				for (&i, &amt) in &burn_results {
					air.adjust_moles(i, amt);
					delta_enthalpy -= amt * gas_info[i as usize].enthalpy;
				}
				air.set_temperature(
					(initial_enthalpy + delta_enthalpy)
						/ (air.heat_capacity() + R_IDEAL_GAS_EQUATION * air.total_moles()),
				);
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
			if radiation_released > 0.0 {
				if let Some(radiation_burn) = Proc::find(byond_string!("/proc/radiation_burn")) {
					radiation_burn.call(&[holder, &Value::from(radiation_released)])?;
				} else {
					drop(Proc::find(byond_string!("/proc/stack_trace"))
					.ok_or_else(|| runtime!("Couldn't find stack_trace!"))?
					.call(&[&Value::from_string(
						"radiation_burn not found! Auxmos hooked fires won't irradiate without it!"
					)?]));
				}
			}
			Ok(Value::from(if fire_amount > 0.0 { 1.0 } else { 0.0 }))
		} else {
			Ok(Value::from(0.0))
		}
	})
}
