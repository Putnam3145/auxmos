use itertools::Itertools;

use std::cell::Cell;

pub use super::simd_vector::*;

use std::ops::{Add, Mul};

use super::{
	constants::*, reaction::ReactionIdentifier, total_num_gases, with_reactions,
	with_specific_heats, GasIDX, VISIBILITIES,
};

/// The data structure representing a Space Station 13 gas mixture.
/// Unlike Monstermos, this doesn't have the archive built-in; instead,
/// the archive is a feature of the turf grid, only existing during
/// turf processing.
/// Also missing is last_share; due to the usage of Rust,
/// processing no longer requires sleeping turfs. Instead, we're using
/// a proper, fully-simulated FDM system, much like LINDA but without
/// sleeping turfs.
#[derive(Clone)]
pub struct GasMixture {
	moles: FlatSimdVec,
	len: usize,
	temperature: f32,
	pub volume: f32,
	min_heat_capacity: f32,
	immutable: bool,
	cached_heat_capacity: Cell<Option<f32>>,
}

/*
	Cell is not thread-safe. However, we use it only for caching heat capacity. The worst case race condition
	is thus thread A and B try to access heat capacity at the same time; both find that it's currently
	uncached, so both go to calculate it; both calculate it, and both calculate it to the same value,
	then one sets the cache to that value, then the other does.

	Technically, a worse one would be thread A mutates the gas mixture, changing a gas amount,
	while thread B tries to get its heat capacity; thread B finds a well-defined heat capacity,
	which is not correct, and uses it for a calculation, but this cannot happen: thread A would
	have a write lock, precluding thread B from accessing it.

	All of the above applies for cached reactions.
*/
unsafe impl Sync for GasMixture {}

impl Default for GasMixture {
	fn default() -> Self {
		Self::new()
	}
}

impl GasMixture {
	/// Makes an empty gas mixture.
	pub fn new() -> Self {
		GasMixture {
			moles: FlatSimdVec::new(),
			len: 0,
			temperature: 2.7,
			volume: 2500.0,
			min_heat_capacity: MINIMUM_HEAT_CAPACITY,
			immutable: false,
			cached_heat_capacity: Cell::new(None),
		}
	}
	/// Makes an empty gas mixture with the given volume.
	pub fn from_vol(vol: f32) -> Self {
		let mut ret: GasMixture = GasMixture::new();
		ret.volume = vol;
		ret
	}
	/// Returns if any data is corrupt.
	pub fn is_corrupt(&self) -> bool {
		!self.temperature.is_normal()
			|| self.moles.len() > total_num_gases()
			|| self.moles.iter().any(|amt| !amt.is_finite().all())
	}
	pub fn gases(&self) -> &FlatSimdVec {
		&self.moles
	}
	/// Fixes any corruption found.
	pub fn fix_corruption(&mut self) {
		self.moles.shrink_to_fit();
		if self.temperature < 2.7 || !self.temperature.is_normal() {
			self.set_temperature(293.15);
		}
	}
	/// Returns the temperature of the mix. T
	pub fn get_temperature(&self) -> f32 {
		self.temperature
	}
	/// Sets the temperature, if the mix isn't immutable. T
	pub fn set_temperature(&mut self, temp: f32) {
		if !self.immutable && temp > 2.7 && temp.is_normal() {
			self.temperature = temp;
		}
	}
	/// Sets the minimum heat capacity of this mix.
	pub fn set_min_heat_capacity(&mut self, amt: f32) {
		self.min_heat_capacity = amt;
	}
	/// Returns (by value) the amount of moles of a given index the mix has. M
	pub fn get_moles(&self, idx: GasIDX) -> f32 {
		self.moles.get(idx).unwrap_or(0.0)
	}
	/// Sets the mix to be internally immutable. Rust doesn't know about any of this, obviously.
	pub fn mark_immutable(&mut self) {
		self.immutable = true;
	}
	/// Returns whether this gas mixture is immutable.
	pub fn is_immutable(&self) -> bool {
		self.immutable
	}
	/// If mix is not immutable, sets the gas at the given `idx` to the given `amt`.
	pub fn set_moles(&mut self, idx: GasIDX, amt: f32) {
		if !self.immutable && idx < total_num_gases() {
			self.moles.force_set(idx, amt.max(0.0));
			self.cached_heat_capacity.set(None);
		}
	}
	pub fn adjust_moles(&mut self, idx: GasIDX, amt: f32) {
		if !self.immutable && amt.is_normal() {
			self.set_moles(idx, self.get_moles(idx) + amt);
			if amt < 0.0 {
				self.moles.shrink_to_fit();
			}
		}
	}
	pub fn adjust_multiple(&mut self, adjustments: &[(GasIDX, f32)]) {
		if !self.immutable {
			self.moles
				.expand(adjustments.iter().map(|a| a.0).max().unwrap_or(0));
			self.moles.with_flat_mut(|gases| {
				for (i, amt) in adjustments.iter().copied() {
					gases[i] += amt;
				}
			});
			self.cached_heat_capacity.set(None);
			self.garbage_collect();
		}
	}
	fn slow_heat_cap(&self) -> f32 {
		let ret = with_specific_heats(|spec_heats| self.moles.dot_product(&spec_heats));
		self.cached_heat_capacity.set(Some(ret));
		ret
	}
	/// The heat capacity of the material. [joules?]/mole-kelvin.
	pub fn heat_capacity(&self) -> f32 {
		self.cached_heat_capacity
			.get()
			.filter(|cap| cap.is_normal())
			.unwrap_or_else(|| self.slow_heat_cap())
	}
	/// Heat capacity of exactly one gas in this mix.
	pub fn partial_heat_capacity(&self, idx: GasIDX) -> f32 {
		self.moles.get(idx).map_or(0.0, |amt| {
			amt * with_specific_heats(|spec| spec.get(idx).unwrap_or(0.0))
		})
	}
	/// A vector of all partial heat capacities.
	pub fn heat_capacities(&self) -> FlatSimdVec {
		with_specific_heats(|spec_heats| &self.moles * spec_heats)
	}
	/// The total mole count of the mixture. Moles.
	pub fn total_moles(&self) -> f32 {
		self.moles.sum()
	}
	/// Pressure. Kilopascals.
	pub fn return_pressure(&self) -> f32 {
		self.total_moles() * R_IDEAL_GAS_EQUATION * self.temperature / self.volume
	}
	/// Thermal energy. Joules?
	pub fn thermal_energy(&self) -> f32 {
		self.heat_capacity() * self.temperature
	}
	pub fn merge_from_vec_with_energy(&mut self, v: &FlatSimdVec, energy: f32) {
		let final_energy = self.thermal_energy() + energy;
		self.moles += v;
		self.cached_heat_capacity.set(None);
		self.set_temperature(final_energy / self.heat_capacity());
	}
	/// Merges one gas mixture into another.
	pub fn merge(&mut self, giver: &GasMixture) {
		if self.immutable || giver.is_corrupt() {
			return;
		}
		let our_heat_capacity = self.heat_capacity();
		let other_heat_capacity = giver.heat_capacity();
		self.moles += &giver.moles;
		let combined_heat_capacity = our_heat_capacity + other_heat_capacity;
		if combined_heat_capacity > MINIMUM_HEAT_CAPACITY {
			self.set_temperature(
				(our_heat_capacity * self.temperature + other_heat_capacity * giver.temperature)
					/ (combined_heat_capacity),
			);
		}
		self.cached_heat_capacity.set(Some(combined_heat_capacity));
	}
	pub fn remove_ratio_into(&mut self, mut ratio: f32, into: &mut GasMixture) {
		if ratio <= 0.0 {
			return;
		}
		if ratio >= 1.0 {
			ratio = 1.0;
		}
		into.copy_from_mutable(self);
		into.multiply(ratio);
		self.multiply(1.0 - ratio);
	}
	pub fn remove_into(&mut self, amount: f32, into: &mut GasMixture) {
		self.remove_ratio_into(amount / self.total_moles(), into);
	}
	/// Returns a gas mixture that contains a given percentage of this mixture's moles; if this mix is mutable, also removes those moles from the original.
	pub fn remove_ratio(&mut self, ratio: f32) -> GasMixture {
		let mut removed = GasMixture::from_vol(self.volume);
		self.remove_ratio_into(ratio, &mut removed);
		removed
	}
	/// As remove_ratio, but a raw number of moles instead of a ratio.
	pub fn remove(&mut self, amount: f32) -> GasMixture {
		self.remove_ratio(amount / self.total_moles())
	}
	/// Copies from a given gas mixture, if we're mutable.
	pub fn copy_from_mutable(&mut self, sample: &GasMixture) {
		if self.immutable && !sample.is_corrupt() {
			return;
		}
		self.moles.copy_from(&sample.moles);
		self.temperature = sample.temperature;
		self.cached_heat_capacity
			.set(sample.cached_heat_capacity.get());
	}
	/// A very simple finite difference solution to the heat transfer equation.
	/// Works well enough for our purposes, though perhaps called less often
	/// than it ought to be while we're working in Rust.
	/// Differs from the original by not using archive, since we don't put the archive into the gas mix itself anymore.
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
					self.set_temperature((self.temperature - heat / self_heat_capacity).max(TCMB));
				}
				if !sharer.immutable {
					sharer.set_temperature(
						(sharer.temperature + heat / sharer_heat_capacity).max(TCMB),
					);
				}
			}
		}
		sharer.temperature
	}
	/// As above, but you may put in any arbitrary coefficient, temp, heat capacity.
	/// Only used for superconductivity as of right now.
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
					self.set_temperature((self.temperature - heat / self_heat_capacity).max(TCMB));
				}
				return (sharer_temperature + heat / sharer_heat_capacity).max(TCMB);
			}
		}
		sharer_temperature
	}
	/// The second part of old compare(). Compares temperature, but only if this gas has sufficiently high moles.
	pub fn temperature_compare(&self, sample: &GasMixture) -> bool {
		(self.get_temperature() - sample.get_temperature()).abs()
			> MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND
			&& (self.total_moles() > MINIMUM_MOLES_DELTA_TO_MOVE)
	}
	/// Returns the maximum mole delta for an individual gas.
	pub fn compare(&self, sample: &GasMixture) -> f32 {
		self.moles
			.iter()
			.copied()
			.zip_longest(sample.moles.iter().copied())
			.fold(SimdGasVec::splat(0.0), |acc, pair| {
				acc + pair.reduce(|a, b| (b - a).abs())
			})
			.max_element()
	}
	pub fn compare_with(&self, sample: &GasMixture, amt: f32) -> bool {
		let real_amt = SimdGasVec::splat(amt);
		self.moles
			.iter()
			.copied()
			.zip_longest(sample.moles.iter().copied())
			.any(|pair| pair.reduce(|a, b| (b - a).abs()).ge(real_amt).any())
	}
	/// Clears the moles from the gas.
	pub fn clear(&mut self) {
		if !self.immutable {
			self.moles.clear();
			self.cached_heat_capacity.set(None);
		}
	}
	/// Resets the gas mixture to an initialized-with-volume state.
	pub fn clear_with_vol(&mut self, vol: f32) {
		self.temperature = 2.7;
		self.volume = vol;
		self.min_heat_capacity = MINIMUM_HEAT_CAPACITY;
		self.immutable = false;
		self.clear();
	}
	/// Multiplies every gas molage with this value.
	pub fn multiply(&mut self, multiplier: f32) {
		if !self.immutable {
			self.cached_heat_capacity
				.set(Some(self.heat_capacity() * multiplier)); // hax
			self.moles *= multiplier;
			self.moles.shrink_to_fit();
		}
	}
	pub fn with_gases<T>(&self, f: impl Fn(&[f32]) -> T) -> T {
		self.moles.with_flat_view(f)
	}
	pub fn get_burnability(&self) -> (f32, f32) {
		(self.get_oxidation_power(), self.get_fuel_amount())
	}
	pub fn get_oxidation_power(&self) -> f32 {
		use super::{OXIDATION_POWERS, OXIDATION_TEMPS};
		let temp_vec = SimdGasVec::splat(self.temperature);
		self.moles
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
			.fold(
				SimdGasVec::splat(0.0),
				|acc, (mut amt, (oxi_temp, oxi_pwr))| {
					amt *= oxi_pwr;
					let temperature_scale = 1.0
						- (temp_vec
							.ge(oxi_temp)
							.select(oxi_temp, SimdGasVec::splat(0.0))
							/ self.temperature);
					amt.mul_adde(temperature_scale, acc)
				},
			)
			.sum()
	}
	pub fn get_fuel_amount(&self) -> f32 {
		use super::{FIRE_RATES, FIRE_TEMPS};
		let temp_vec = SimdGasVec::splat(self.temperature);
		self.moles
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
			.fold(
				SimdGasVec::splat(0.0),
				|acc, (mut amt, (fire_temp, fire_burn_rate))| {
					amt /= fire_burn_rate;
					let temperature_scale = 1.0
						- (temp_vec
							.ge(fire_temp)
							.select(fire_temp, SimdGasVec::splat(0.0))
							/ self.temperature);
					amt.mul_adde(temperature_scale, acc)
				},
			)
			.sum()
	}
	/// Checks if the gas can react with any reactions.
	pub fn can_react(&self) -> bool {
		!self.immutable
			&& with_reactions(|reactions| reactions.iter().any(|r| r.check_conditions(self)))
	}
	/// Gets all of the reactions this mix should do.
	pub fn all_reactable(&self) -> Option<Vec<ReactionIdentifier>> {
		(!self.immutable).then(|| {
			with_reactions(|reactions| {
				reactions
					.iter()
					.filter_map(|r| {
						if r.check_conditions(self) {
							Some(r.get_id())
						} else {
							None
						}
					})
					.collect()
			})
		})
	}
	/// Adds heat directly to the gas mixture, in joules (probably).
	pub fn adjust_heat(&mut self, heat: f32) {
		let cap = self.heat_capacity();
		self.set_temperature(((cap * self.temperature) + heat) / cap);
	}
	/// Returns true if there's a visible gas in this mix.
	pub fn is_visible(&self) -> bool {
		self.gases()
			.iter()
			.copied()
			.zip(VISIBILITIES.read().as_ref().unwrap().iter().copied())
			.any(|(a, b)| a.ge(b).any())
	}
	/// A hashed representation of the visibility of a gas, so that it only needs to update vis when actually changed.
	pub fn visibility_hash(&self, gas_visibility: &[Option<f32>]) -> u64 {
		use std::hash::Hasher;
		let mut hasher: fxhash::FxHasher64 = Default::default();
		for i in 0..self.moles.len() {
			let gas = self.get_moles(i);
			if let Some(amt) = unsafe { gas_visibility.get_unchecked(i) }.filter(|&amt| gas >= amt)
			{
				hasher.write_usize(i);
				hasher.write_usize((FACTOR_GAS_VISIBLE_MAX).min((gas / amt).ceil()) as GasIDX);
			}
		}
		hasher.finish()
	}
	pub fn garbage_collect(&mut self) {
		self.moles.shrink_to_fit();
	}
}

/// Takes a copy of the mix, merges the right hand side, then returns the copy.
impl Add<&GasMixture> for GasMixture {
	type Output = Self;

	fn add(self, rhs: &GasMixture) -> Self {
		let mut ret = self.clone();
		ret.merge(rhs);
		ret
	}
}

/// Takes a copy of the mix, merges the right hand side, then returns the copy.
impl<'a, 'b> Add<&'a GasMixture> for &'b GasMixture {
	type Output = GasMixture;

	fn add(self, rhs: &GasMixture) -> GasMixture {
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

/// Makes a copy of the given mix, multiplied by a scalar.
impl<'a> Mul<f32> for &'a GasMixture {
	type Output = GasMixture;

	fn mul(self, rhs: f32) -> GasMixture {
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
		assert!(
			(into.get_temperature() - 311.266).abs() < 0.01,
			"{} should be near 311.266, is {}",
			into.get_temperature(),
			(into.get_temperature() - 311.266)
		);
	}
	#[test]
	fn test_remove() {
		// also tests multiply, copy_from_mutable
		let mut removed = GasMixture::new();
		removed.set_moles(0, 22.0);
		removed.set_moles(1, 82.0);
		let new = removed.remove_ratio(0.5);
		assert_eq!(removed.compare(&new) >= MINIMUM_MOLES_DELTA_TO_MOVE, false);
		assert_eq!(removed.get_moles(0), 11.0);
		assert_eq!(removed.get_moles(1), 41.0);
		removed.mark_immutable();
		let new_two = removed.remove_ratio(0.5);
		assert_eq!(
			removed.compare(&new_two) >= MINIMUM_MOLES_DELTA_TO_MOVE,
			true
		);
		assert_eq!(removed.get_moles(0), 11.0);
		assert_eq!(removed.get_moles(1), 41.0);
		assert_eq!(new_two.get_moles(0), 5.5);
	}
}
