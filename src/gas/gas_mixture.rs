use itertools::Itertools;

use std::cell::Cell;

use tinyvec::TinyVec;

use itertools::EitherOrBoth::{Both, Left, Right};

use super::reaction::ReactionIdentifier;

use super::constants::*;

use super::{gas_specific_heat, gas_visibility, total_num_gases, with_reactions, GasIDX};

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
	temperature: f32,
	pub volume: f32,
	min_heat_capacity: f32,
	immutable: bool,
	moles: TinyVec<[f32; 8]>,
	heat_capacities: TinyVec<[f32; 8]>,
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
			moles: TinyVec::new(),
			heat_capacities: TinyVec::new(),
			temperature: 2.7,
			volume: 2500.0,
			min_heat_capacity: 0.0,
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
			|| self.moles.len() < self.heat_capacities.len()
	}
	/// Fixes any corruption found.
	pub fn fix_corruption(&mut self) {
		self.garbage_collect();
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
		if !self.immutable && temp.is_normal() {
			self.temperature = temp;
		}
	}
	/// Sets the minimum heat capacity of this mix.
	pub fn set_min_heat_capacity(&mut self, amt: f32) {
		self.min_heat_capacity = amt;
	}
	/// Returns an iterator over the gas keys and mole amounts thereof.
	pub fn enumerate(&self) -> impl Iterator<Item = (GasIDX, f32)> + '_ {
		self.moles.iter().copied().enumerate()
	}
	/// Same as above, but with heat included.
	pub fn enumerate_with_heat(&self) -> impl Iterator<Item = (GasIDX, f32, f32)> + '_ {
		self.enumerate()
			.zip(self.heat_capacities.iter().copied())
			.map(|((a, b), c)| (a, b, c))
	}
	/// Allows closures to iterate over each gas.
	pub fn for_each_gas(
		&self,
		mut f: impl FnMut(GasIDX, f32) -> Result<(), auxtools::Runtime>,
	) -> Result<(), auxtools::Runtime> {
		for (i, g) in self.enumerate() {
			f(i, g)?;
		}
		Ok(())
	}
	/// Returns (by value) the amount of moles of a given index the mix has. M
	pub fn get_moles(&self, idx: GasIDX) -> f32 {
		*self.moles.get(idx).unwrap_or(&0.0)
	}
	/// Sets the mix to be internally immutable. Rust doesn't know about any of this, obviously.
	pub fn mark_immutable(&mut self) {
		self.immutable = true;
	}
	/// Returns whether this gas mixture is immutable.
	pub fn is_immutable(&self) -> bool {
		self.immutable
	}
	fn maybe_expand(&mut self, size: usize) {
		if self.moles.len() < size {
			self.moles.resize(size, 0.0);
			self.heat_capacities
				.extend((self.heat_capacities.len()..size).map(|i| gas_specific_heat(i)));
		}
	}
	/// If mix is not immutable, sets the gas at the given `idx` to the given `amt`.
	pub fn set_moles(&mut self, idx: GasIDX, amt: f32) {
		if !self.immutable && idx < total_num_gases() {
			if idx <= self.moles.len() || (amt.is_normal() && amt > GAS_MIN_MOLES) {
				self.maybe_expand((idx + 1) as usize);
				unsafe { *self.moles.get_unchecked_mut(idx) = amt };
				self.cached_heat_capacity.set(None);
			}
		}
	}
	pub fn adjust_moles(&mut self, idx: GasIDX, amt: f32) {
		if !self.immutable && amt.is_normal() && idx < total_num_gases() {
			self.maybe_expand((idx + 1) as usize);
			let r = unsafe { self.moles.get_unchecked_mut(idx) };
			*r += amt;
			if amt < 0.0 {
				self.garbage_collect();
			}
			self.cached_heat_capacity.set(None);
		}
	}
	/// The heat capacity of the material. [joules?]/mole-kelvin.
	pub fn heat_capacity(&self) -> f32 {
		if let Some(heat_cap) = self
			.cached_heat_capacity
			.get()
			.filter(|cap| cap.is_normal())
		{
			return heat_cap;
		}
		let heat_cap = self
			.enumerate_with_heat()
			.fold(0.0, |acc, (_, amt, cap)| amt.mul_add(cap, acc))
			.max(self.min_heat_capacity);
		self.cached_heat_capacity.set(Some(heat_cap));
		heat_cap
	}
	/// Heat capacity of exactly one gas in this mix.
	pub fn partial_heat_capacity(&self, idx: GasIDX) -> f32 {
		self.moles.get(idx).map_or(0.0, |amt| {
			amt * unsafe { self.heat_capacities.get_unchecked(idx) }
		})
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
		let our_heat_capacity = self.heat_capacity();
		let other_heat_capacity = giver.heat_capacity();
		self.maybe_expand(giver.moles.len());
		for (id, amt) in giver.enumerate() {
			unsafe { *self.moles.get_unchecked_mut(id) += amt };
		}
		let combined_heat_capacity = our_heat_capacity + other_heat_capacity;
		if combined_heat_capacity > MINIMUM_HEAT_CAPACITY {
			self.set_temperature(
				(our_heat_capacity * self.temperature + other_heat_capacity * giver.temperature)
					/ (combined_heat_capacity),
			);
		}
		self.cached_heat_capacity.set(Some(combined_heat_capacity));
	}
	pub fn transfer_gases_to(&mut self, r: f32, gases: &[GasIDX], into: &mut GasMixture) {
		let ratio = r.clamp(0.0, 1.0);
		let initial_energy = into.thermal_energy();
		let mut heat_transfer = 0.0;
		for i in gases.iter().copied() {
			let orig = self.get_moles(i);
			if orig > 0.0 {
				let delta = orig * ratio;
				heat_transfer += delta * self.temperature * self.heat_capacities[i];
				*unsafe { self.moles.get_unchecked_mut(i) } -= delta;
				into.adjust_moles(i, delta);
			}
		}
		self.cached_heat_capacity.set(None);
		into.cached_heat_capacity.set(None);
		into.set_temperature((initial_energy + heat_transfer) / into.heat_capacity());
	}
	pub fn remove_ratio_into(&mut self, mut ratio: f32, into: &mut GasMixture) {
		if ratio <= 0.0 {
			return;
		}
		if ratio >= 1.0 {
			ratio = 1.0;
		}
		let orig_temp = self.temperature;
		into.copy_from_mutable(self);
		into.multiply(ratio);
		self.multiply(1.0 - ratio);
		self.temperature = orig_temp;
		into.temperature = orig_temp;
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
		if self.immutable {
			return;
		}
		self.moles = sample.moles.clone();
		self.temperature = sample.temperature;
		self.heat_capacities = sample.heat_capacities.clone();
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
			.fold(0.0, |acc, pair| acc.max(pair.reduce(|a, b| (b - a).abs())))
	}
	pub fn compare_with(&self, sample: &GasMixture, amt: f32) -> bool {
		self.moles
			.iter()
			.copied()
			.zip_longest(sample.moles.iter().copied())
			.any(|pair| match pair {
				Left(a) => a >= amt,
				Right(b) => b >= amt,
				Both(a, b) => a != b && (a - b).abs() >= amt,
			})
	}
	/// Clears the moles from the gas.
	pub fn clear(&mut self) {
		if !self.immutable {
			self.moles.clear();
			self.heat_capacities.clear();
			self.cached_heat_capacity.set(None);
		}
	}
	/// Resets the gas mixture to an initialized-with-volume state.
	pub fn clear_with_vol(&mut self, vol: f32) {
		self.temperature = 2.7;
		self.volume = vol;
		self.min_heat_capacity = 0.0;
		self.immutable = false;
		self.clear();
	}
	/// Multiplies every gas molage with this value.
	pub fn multiply(&mut self, multiplier: f32) {
		if !self.immutable {
			for amt in self.moles.iter_mut() {
				*amt *= multiplier;
			}
			self.cached_heat_capacity.set(None);
			self.garbage_collect();
		}
	}
	/// Checks if the proc can react with any reactions.
	pub fn can_react(&self) -> bool {
		with_reactions(|reactions| reactions.iter().any(|r| r.check_conditions(self)))
	}
	/// Gets all of the reactions this mix should do.
	pub fn all_reactable(&self) -> Vec<ReactionIdentifier> {
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
	}
	pub fn get_burnability(&self) -> (f32, f32) {
		super::with_gas_info(|gas_info| {
			self.enumerate().fold((0.0, 0.0), |mut acc, (i, amt)| {
				let this_gas_info = &gas_info[i as usize];
				if let Some(oxidation) = this_gas_info.oxidation {
					if self.temperature > oxidation.temperature() {
						let amount =
							amt * (1.0 - oxidation.temperature() / self.temperature).max(0.0);
						acc.0 += amount * oxidation.power();
					}
				} else if let Some(fire) = this_gas_info.fire {
					if self.temperature > fire.temperature() {
						let amount = amt * (1.0 - fire.temperature() / self.temperature).max(0.0);
						acc.1 += amount / fire.burn_rate();
					}
				}
				acc
			})
		})
	}
	pub fn get_oxidation_power(&self) -> f32 {
		self.get_burnability().0
	}
	pub fn get_fuel_amount(&self) -> f32 {
		self.get_burnability().1
	}
	pub fn get_fire_info_with_lock(
		&self,
		gas_info: &Vec<super::GasType>,
	) -> (Vec<(usize, f32, f32)>, Vec<(usize, f32, f32)>) {
		let (mut fuels, oxidizers): (Vec<_>, Vec<_>) = self
			.enumerate()
			.filter_map(|(i, g)| {
				let this_gas_info = &gas_info[i as usize];
				if let Some(oxidation) = this_gas_info.oxidation {
					if self.get_temperature() > oxidation.temperature() {
						let amount =
							g * (1.0 - oxidation.temperature() / self.get_temperature()).max(0.0);
						Some((i, amount, amount * oxidation.power()))
					} else {
						None
					}
				} else if let Some(fire) = this_gas_info.fire {
					if self.get_temperature() > fire.temperature() {
						let amount =
							g * (1.0 - fire.temperature() / self.get_temperature()).max(0.0);
						Some((i, amount, -amount / fire.burn_rate()))
					} else {
						None
					}
				} else {
					None
				}
			})
			.partition(|&(_, _, power)| power < 0.0);
		for (_, _, power) in fuels.iter_mut() {
			*power *= -1.0;
		}
		(fuels, oxidizers)
	}
	pub fn get_fire_info(&self) -> (Vec<(usize, f32, f32)>, Vec<(usize, f32, f32)>) {
		super::with_gas_info(|gas_info| self.get_fire_info_with_lock(gas_info))
	}
	/// Adds heat directly to the gas mixture, in joules (probably).
	pub fn adjust_heat(&mut self, heat: f32) {
		let cap = self.heat_capacity();
		self.set_temperature(((cap * self.temperature) + heat) / cap);
	}
	/// Returns true if there's a visible gas in this mix.
	pub fn is_visible(&self) -> bool {
		self.enumerate()
			.any(|(i, gas)| gas_visibility(i as usize).map_or(false, |amt| gas >= amt))
	}
	/// A hashed representation of the visibility of a gas, so that it only needs to update vis when actually changed.
	pub fn visibility_hash(&self, gas_visibility: &[Option<f32>]) -> u64 {
		use std::hash::Hasher;
		let mut hasher: fxhash::FxHasher64 = Default::default();
		for (i, gas) in self.enumerate() {
			if let Some(amt) = unsafe { gas_visibility.get_unchecked(i) }.filter(|&amt| gas >= amt)
			{
				hasher.write_usize(i);
				hasher.write_usize((FACTOR_GAS_VISIBLE_MAX).min((gas / amt).ceil()) as GasIDX);
			}
		}
		hasher.finish()
	}
	// Removes all zeroes from the gas mixture.
	pub fn garbage_collect(&mut self) {
		let mut last_valid_found = 0;
		for (i, amt) in self.moles.iter_mut().enumerate() {
			if *amt > GAS_MIN_MOLES {
				last_valid_found = i;
			} else {
				*amt = 0.0;
			}
		}
		self.moles.truncate(last_valid_found + 1);
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
