extern crate sprs;

use itertools::Itertools;

use super::constants::*;

use super::gas_specific_heats;

/**
 * The data structure representing a Space Station 13 gas mixture.
 * Unlike Monstermos, this doesn't have the archive built-in; instead,
 * the archive is a feature of the turf grid, only existing during
 * turf processing.
 * Also missing is last_share; due to the usage of Rust,
 * processing no longer requires sleeping turfs. Instead, we're using
 * a proper, fully-simulated FDM system, much like LINDA but without
 * sleeping turfs.
*/
#[derive(Clone)]
pub struct GasMixture {
	moles: Vec<f32>,
	temperature: f32,
	pub volume: f32,
	pub min_heat_capacity: f32,
	immutable: bool,
}

impl GasMixture {
	/// Makes an empty gas mixture.
	pub fn new() -> Self {
		GasMixture {
			moles: Vec::new(),
			temperature: 0.0,
			volume: 0.0,
			min_heat_capacity: 0.0,
			immutable: false,
		}
	}
	/// Makes an empty gas mixture with the given volume.
	pub fn from_vol(vol: f32) -> Self {
		let mut ret: GasMixture = GasMixture::new();
		ret.volume = vol;
		ret
	}
	/// Returns the temperature of the mix. T
	pub fn get_temperature(&self) -> f32 {
		self.temperature
	}
	/// Sets the temperature, if the mix isn't immutable. T
	pub fn set_temperature(&mut self, temp: f32) {
		if !self.immutable {
			self.temperature = temp;
		}
	}
	/// Returns a slice representing the non-zero gases in the mix.
	pub fn get_gases(&self) -> Vec<usize> {
		self.moles
			.iter()
			.enumerate()
			.filter_map(|(i, n)| if *n > 0.0 { Some(i) } else { None })
			.collect()
	}
	/// Returns (by value) the amount of moles of a given index the mix has. M
	pub fn get_moles(&self, idx: usize) -> f32 {
		self.moles[idx]
	}
	/// Sets the mix to be internally immutable. Rust doesn't know about any of this, obviously.
	pub fn mark_immutable(&mut self) {
		self.immutable = true;
	}
	/// If mix is not immutable, sets the gas at the given `idx` to the given `amt`.
	pub fn set_moles(&mut self, idx: usize, amt: f32) {
		if !self.immutable {
			if self.moles.len() <= idx {
				self.moles.resize(idx + 1, 0.0);
			}
			self.moles[idx] = amt;
		}
	}
	/// The heat capacity of the material. [joules?]/mole-kelvin.
	pub fn heat_capacity(&self) -> f32 {
		self.moles
			.iter()
			.zip(gas_specific_heats().iter())
			.fold(0.0, |acc, (gas, cap)| acc + (gas * cap))
	}
	/// The total mole count of the mixture. Moles.
	pub fn total_moles(&self) -> f32 {
		self.moles.iter().sum()
	}
	/// Pressure. Kilopascals.
	pub fn return_pressure(&self) -> f32 {
		self.total_moles() * R_IDEAL_GAS_EQUATION * self.temperature / self.volume
	}
	/// Thermal energy. Joules?
	pub fn thermal_energy(&self) -> f32 {
		self.heat_capacity() * self.temperature
	}
	/// Merges one gas mixture into another.
	pub fn merge(&mut self, giver: &GasMixture) {
		if self.immutable {
			return;
		}
		let tot_energy = self.thermal_energy() + giver.thermal_energy();
		self.moles = self
			.moles
			.iter()
			.copied()
			.zip_longest(giver.moles.iter().copied())
			.map(|i| (i.reduce(|a, b| a + b)))
			.collect();
		self.temperature = tot_energy / self.heat_capacity();
	}
	/// Returns a gas mixture that contains a given percentage of this mixture's moles; if this mix is mutable, also removes those moles from the original.
	pub fn remove_ratio(&mut self, mut ratio: f32) -> GasMixture {
		let mut removed = GasMixture::from_vol(self.volume);
		if ratio <= 0.0 {
			return removed;
		}
		if ratio >= 1.0 {
			ratio = 1.0;
		}
		removed.copy_from_mutable(self);
		removed.multiply(ratio);
		if !self.immutable {
			for (our_moles, removed_moles) in self.moles.iter_mut().zip(removed.moles.iter()) {
				*our_moles -= removed_moles;
			}
		}
		removed
	}
	/// As remove_ratio, but a raw number of moles instead of a ratio.
	pub fn remove(&mut self, amount: f32) -> GasMixture {
		self.remove_ratio(amount / self.total_moles())
	}
	/// Copies from a given gas mixture, if we're mutable.
	pub fn copy_from_mutable(&mut self, sample: &GasMixture) {
		if self.immutable {
			return;
		}
		self.moles = sample.moles.clone();
		self.temperature = sample.temperature;
	}
	/**
	 * A naive solution to the discrete poisson equation, used for processing turf atmospherics.
	 * Yeah, turf atmospherics is just diffusion by default with LINDA, plus eventually skipping the rest with excited groups.
	 * Assuming those work, of course.
	 * Shouldn't diverge due to the atmos_adjacent_turfs+1 denominator, but you can't trust these things.
	 * It seems to have worked all this time, though.
	 *
	 */
	#[deprecated(since = "0.1.0", note = "Replaced by TurfGrid::process_turfs")]
	fn share(&mut self, sharer: &mut GasMixture, atmos_adjacent_turfs: i32) -> f32 {
		/*
		let temperature_delta = self.temperature_archived - sharer.temperature_archived;
		let abs_temperature_delta = temperature_delta.abs();
		let old_self_heat_capacity: f32;
		let old_sharer_heat_capacity: f32;
		if abs_temperature_delta > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
			old_self_heat_capacity = self.heat_capacity();
			old_sharer_heat_capacity = sharer.heat_capacity();
		} else {
			old_self_heat_capacity = 0.0;
			old_sharer_heat_capacity = 0.0;
		}
		let mut moved_moles: f32 = 0.0;
		let mut abs_moved_moles: f32 = 0.0;

		let deltas = &self.moles_archived + -sharer.moles_archived.clone();
		let heat_caps: Vec<f32> = deltas
			.iter()
			.map(|(idx, val)| val * gas_specific_heats()[idx])
			.collect();
		if !self.immutable {
			self.moles = &self.moles + -deltas.clone();
		}
		if !sharer.immutable {
			sharer.moles = &sharer.moles + &deltas;
		}
		moved_moles = deltas.data().iter().sum();
		abs_moved_moles = deltas.data().iter().map(|a| a.abs()).sum();
		let heat_capacity_self_to_sharer: f32 = heat_caps
			.iter()
			.map(|&a| if a > 0.0 { a } else { 0.0 })
			.sum();
		let heat_capacity_sharer_to_self: f32 = heat_caps
			.iter()
			.map(|&a| if a < 0.0 { -a } else { 0.0 })
			.sum();
		self.last_share = abs_moved_moles;

		if abs_temperature_delta > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
			let new_self_heat_capacity = old_self_heat_capacity + heat_capacity_sharer_to_self
				- heat_capacity_self_to_sharer;
			let new_sharer_heat_capacity = old_sharer_heat_capacity + heat_capacity_self_to_sharer
				- heat_capacity_sharer_to_self;

			//transfer of thermal energy (via changed heat capacity) between self and sharer
			if !self.immutable && new_self_heat_capacity > MINIMUM_HEAT_CAPACITY {
				self.temperature = (old_self_heat_capacity * self.temperature
					- heat_capacity_self_to_sharer * self.temperature_archived
					+ heat_capacity_sharer_to_self * sharer.temperature_archived)
					/ new_self_heat_capacity;
			}

			if !sharer.immutable && new_sharer_heat_capacity > MINIMUM_HEAT_CAPACITY {
				sharer.temperature = (old_sharer_heat_capacity * sharer.temperature
					- heat_capacity_sharer_to_self * sharer.temperature_archived
					+ heat_capacity_self_to_sharer * self.temperature_archived)
					/ new_sharer_heat_capacity;
			}
			//thermal energy of the system (self and sharer) is unchanged

			if old_sharer_heat_capacity.abs() > MINIMUM_HEAT_CAPACITY {
				if (new_sharer_heat_capacity / old_sharer_heat_capacity - 1.0).abs() < 0.1 {
					// <10% change in sharer heat capacity
					self.temperature_share(sharer, OPEN_HEAT_TRANSFER_COEFFICIENT);
				}
			}
		}
		if temperature_delta > MINIMUM_TEMPERATURE_TO_MOVE
			|| abs_moved_moles > MINIMUM_MOLES_DELTA_TO_MOVE
		{
			let our_moles = self.total_moles();
			let their_moles = sharer.total_moles();
			return (self.temperature_archived * (our_moles + moved_moles)
				- sharer.temperature_archived * (their_moles - moved_moles))
				* R_IDEAL_GAS_EQUATION
				/ self.volume;
		}
		*/
		0.0
	}
	/**
	 * A very simple finite difference solution to the heat transfer equation.
	 * Works well enough for our purposes, though perhaps called less often
	 * than it ought to be while we're working in Rust.
	 * Differs from the original by not using archive, since we don't put the archive into the gas mix itself anymore.
	 */
	pub fn temperature_share(
		&mut self,
		sharer: &mut GasMixture,
		conduction_coefficient: f32,
	) -> f32 {
		let temperature_delta = self.temperature - sharer.temperature;
		if temperature_delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
			let self_heat_capacity = self.heat_capacity();
			let sharer_heat_capacity = sharer.heat_capacity();

			if sharer_heat_capacity > MINIMUM_HEAT_CAPACITY
				&& self_heat_capacity > MINIMUM_HEAT_CAPACITY
			{
				let heat = conduction_coefficient
					* temperature_delta * (self_heat_capacity * sharer_heat_capacity
					/ (self_heat_capacity + sharer_heat_capacity));
				if !self.immutable {
					self.temperature = (self.temperature - heat / self_heat_capacity).max(TCMB);
				}
				if !sharer.immutable {
					sharer.temperature =
						(sharer.temperature + heat / sharer_heat_capacity).max(TCMB);
				}
			}
		}
		sharer.temperature
	}
	/*
	 * As above, but you may put in any arbitrary coefficient, temp, heat capacity.
	 * Only used for superconductivity as of right now.
	 */
	pub fn temperature_share_non_gas(
		&mut self,
		conduction_coefficient: f32,
		sharer_temperature: f32,
		sharer_heat_capacity: f32,
	) -> f32 {
		let temperature_delta = self.temperature - sharer_temperature;
		if temperature_delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
			let self_heat_capacity = self.heat_capacity();

			if sharer_heat_capacity > MINIMUM_HEAT_CAPACITY
				&& self_heat_capacity > MINIMUM_HEAT_CAPACITY
			{
				let heat = conduction_coefficient
					* temperature_delta * (self_heat_capacity * sharer_heat_capacity
					/ (self_heat_capacity + sharer_heat_capacity));
				if !self.immutable {
					self.temperature = (self.temperature - heat / self_heat_capacity).max(TCMB);
				}
				return (sharer_temperature + heat / sharer_heat_capacity).max(TCMB);
			}
		}
		sharer_temperature
	}
	/// Returns -2 if gases are extremely similar, -1 if they have a temp difference, otherwise index of first gas with large difference found.
	pub fn compare(&self, sample: &GasMixture) -> i32 {
		for (i, (our_moles, their_moles)) in self.moles.iter().zip(sample.moles.iter()).enumerate()
		{
			let delta = our_moles - their_moles;
			if delta > MINIMUM_MOLES_DELTA_TO_MOVE && delta > our_moles * MINIMUM_AIR_RATIO_TO_MOVE
			{
				return i as i32;
			}
		}
		if self.total_moles() > MINIMUM_MOLES_DELTA_TO_MOVE {
			if (self.temperature - sample.temperature).abs() > MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND
			{
				return -1;
			}
		}
		-2
	}
	/// Clears the moles from the gas.
	pub fn clear(&mut self) {
		if !self.immutable {
			self.moles.clear();
		}
	}
	/// Multiplies every gas molage with this value.
	pub fn multiply(&mut self, multiplier: f32) {
		if !self.immutable {
			for amt in self.moles.iter_mut() {
				*amt *= multiplier;
			}
		}
	}
}

use std::ops::{Add, Mul};

/// Takes a copy of the mix, merges the right hand side, then returns the copy.
impl Add<&GasMixture> for GasMixture {
	type Output = Self;

	fn add(self, rhs: &GasMixture) -> Self {
		let mut ret = self.clone();
		ret.merge(rhs);
		ret
	}
}

/// Makes a copy of the given mix, multiplied by a scalar.
impl Mul<f32> for GasMixture {
	type Output = Self;

	fn mul(self, rhs: f32) -> Self {
		let mut ret = self.clone();
		ret.multiply(rhs);
		ret
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_merge() {
		let mut into = GasMixture::new();
		into.set_moles(0, 82.0);
		into.set_moles(1, 22.0);
		into.set_temperature(293.15);
		let mut source = GasMixture::new();
		source.set_moles(3, 100.0);
		source.set_temperature(313.15);
		into.merge(&source);
		// make sure that the merge successfuly moved the moles
		assert_eq!(into.get_moles(3), 100.0);
		assert_eq!(source.get_moles(3), 100.0); // source is not modified by merge
										/*
										make sure that the merge successfuly changed the temperature of the mix merged into
										test gases have heat capacities of 2,080 and 20,000 respectively, so total thermal energies of
										609,752 and 6,263,000 respectively once multiplied by temperatures. add those together,
										then divide by new total heat capacity:
										(609,752 + 6,263,000)/(2,080 + 20,000) =
										6,872,752 / 2,2080 ~
										311.265942
										so we compare to see if it's relatively close to 311.266, cause of floating point precision
										*/
		assert!((into.get_temperature() - 313.266).abs() < 0.01);
	}
}
