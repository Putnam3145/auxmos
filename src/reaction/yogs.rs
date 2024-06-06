use crate::gas::{constants::*, gas_idx_from_string, with_mix, with_mix_mut};
use byondapi::prelude::*;
use eyre::Result;

#[must_use]
pub fn func_from_id(id: &str) -> Option<super::ReactFunc> {
	match id {
		"plasmafire" => Some(plasma_fire),
		//"tritfire" => Some(tritium_fire),
		//"fusion" => Some(fusion),
		_ => None,
	}
}

const SUPER_SATURATION_THRESHOLD: f32 = 96.0;
fn plasma_fire(byond_air: ByondValue, holder: ByondValue) -> Result<ByondValue> {
	const PLASMA_UPPER_TEMPERATURE: f32 = 1370.0 + T0C;
	const OXYGEN_BURN_RATE_BASE: f32 = 1.4;
	const PLASMA_OXYGEN_FULLBURN: f32 = 10.0;
	const PLASMA_BURN_RATE_DELTA: f32 = 9.0;
	const FIRE_PLASMA_ENERGY_RELEASED: f32 = 3_000_000.0;
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
					(air.get_temperature() - PLASMA_MINIMUM_BURN_TEMPERATURE)
						/ (PLASMA_UPPER_TEMPERATURE - PLASMA_MINIMUM_BURN_TEMPERATURE)
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
		let mut cached_results = byond_air.read_var_id(byond_string!("reaction_results"))?;
		cached_results.write_list_index("fire", fire_amount)?;
		if temperature > FIRE_MINIMUM_TEMPERATURE_TO_EXIST {
			byondapi::global_call::call_global_id(
				byond_string!("fire_expose"),
				&[holder, byond_air, temperature.into()],
			)?;
		}
		Ok(true.into())
	} else {
		Ok(false.into())
	}
}
/*
fn tritium_fire(byond_air: ByondValue, holder: ByondValue) -> Result<ByondValue> {
	Ok(ByondValue::new())
}

fn fusion(byond_air: ByondValue, holder: ByondValue) -> Result<ByondValue> {
	Ok(ByondValue::new())
}
*/
