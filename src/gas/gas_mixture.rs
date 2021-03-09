use itertools::Itertools;

use itertools::EitherOrBoth::{Both, Left, Right};

use tinyvec::TinyVec;

use std::cmp::Ordering;

use super::constants::*;

use super::{gas_specific_heats, gas_visibility, reactions, total_num_gases};

use super::reaction::Reaction;

trait LinearSearch<T> {
	fn linear_search(&self, x: &T) -> Result<usize, usize>
	where
		T: Ord;
	fn linear_search_by<F>(&self, f: F) -> Result<usize, usize>
	where
		F: FnMut(&T) -> Ordering;
	fn linear_search_by_key<B, F>(&self, b: &B, f: F) -> Result<usize, usize>
	where
		B: Ord,
		F: FnMut(&T) -> B;
}

impl<T> LinearSearch<T> for [T] {
	fn linear_search(&self, x: &T) -> Result<usize, usize>
	where
		T: Ord,
	{
		self.linear_search_by(|p| p.cmp(x))
	}
	fn linear_search_by<F>(&self, mut f: F) -> Result<usize, usize>
	where
		F: FnMut(&T) -> Ordering,
	{
		for (idx, i) in self.iter().enumerate() {
			match f(i) {
				Ordering::Equal => return Ok(idx),
				Ordering::Greater => return Err(idx),
				_ => (),
			}
		}
		Err(self.len())
	}
	fn linear_search_by_key<B, F>(&self, b: &B, mut f: F) -> Result<usize, usize>
	where
		B: Ord,
		F: FnMut(&T) -> B,
	{
		self.linear_search_by(|k| f(k).cmp(b))
	}
}

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
	mole_ids: TinyVec<[u8; 8]>,
	moles: TinyVec<[f32; 8]>,
}

impl Default for GasMixture {
	fn default() -> Self {
		Self::new()
	}
}

impl GasMixture {
	/// Makes an empty gas mixture.
	pub fn new() -> Self {
		GasMixture {
			mole_ids: TinyVec::new(),
			moles: TinyVec::new(),
			temperature: 2.7,
			volume: 2500.0,
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
		if !self.immutable && temp.is_normal() {
			self.temperature = temp;
		}
	}
	/// Sets the minimum heat capacity of this mix.
	pub fn set_min_heat_capacity(&mut self, amt: f32) {
		self.min_heat_capacity = amt;
	}
	// Returns an iterator over the gas keys and mole amounts thereof.
	fn enumerate(&self) -> impl Iterator<Item = (&u8, &f32)> {
		self.mole_ids.iter().zip_eq(self.moles.iter())
	}
	/// Allows closures to iterate over each gas.
	pub fn for_each_gas<F>(&self, f: F)
	where
		F: FnMut((&u8, &f32)),
	{
		self.enumerate().for_each(f);
	}
	/// Returns a slice representing the non-zero gases in the mix.
	pub fn get_gases(&self) -> &[u8] {
		&self.mole_ids
	}
	/// Returns (by value) the amount of moles of a given index the mix has. M
	pub fn get_moles(&self, idx: u8) -> f32 {
		if let Ok(i) = self.mole_ids.linear_search(&idx) {
			*unsafe { self.moles.get_unchecked(i) } // mole_ids and moles have same size, so `i` is always in bounds
		} else {
			0.0
		}
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
	pub fn set_moles(&mut self, idx: u8, amt: f32) {
		if !self.immutable && idx < total_num_gases() {
			if amt.is_normal() && amt > GAS_MIN_MOLES {
				match self.mole_ids.linear_search(&idx) {
					Ok(i) => unsafe { *self.moles.get_unchecked_mut(i) = amt },
					Err(i) => {
						self.mole_ids.insert(i, idx);
						self.moles.insert(i, amt);
					}
				}
			} else {
				if let Ok(i) = self.mole_ids.linear_search(&idx) {
					self.moles.remove(i);
					self.mole_ids.remove(i);
				}
			}
		}
	}
	/// The heat capacity of the material. [joules?]/mole-kelvin.
	#[inline]
	pub fn heat_capacity(&self) -> f32 {
		let heats = gas_specific_heats();
		self.enumerate()
			.fold(0.0, |acc, (&id, amt)| {
				acc + amt * unsafe { heats.get_unchecked(id as usize) }
			})
			.max(self.min_heat_capacity)
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
		for (id, amt) in giver.enumerate() {
			match self.mole_ids.linear_search(id) {
				Ok(idx) => unsafe { *self.moles.get_unchecked_mut(idx) += amt },
				Err(idx) => {
					self.moles.insert(idx, *amt);
					self.mole_ids.insert(idx, *id);
				}
			}
		}
		if our_heat_capacity + other_heat_capacity > MINIMUM_HEAT_CAPACITY {
			self.set_temperature(
				(our_heat_capacity * self.temperature + other_heat_capacity * giver.temperature)
					/ (our_heat_capacity + other_heat_capacity),
			);
		}
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
		self.multiply(1.0 - ratio);
		self.garbage_collect();
		removed.garbage_collect();
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
		self.mole_ids = sample.mole_ids.clone();
		self.moles = sample.moles.clone();
		self.temperature = sample.temperature;
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
	/// Returns true if the gases are sufficiently different, false otherwise.
	pub fn compare(&self, sample: &GasMixture, min_delta: f32) -> bool {
		let mut our_idx = 0;
		let mut sample_idx = 0;
		// https://docs.rs/itertools/0.10.0/itertools/trait.Itertools.html#method.merge_join_by
		// Walks linearly through each gas mixture, using the above method.
		// We keep track of the indices of each as we go.
		// The Left(_) and Right(_) paths have an implicit subtract-by-0 in them.
		for e in self
			.mole_ids
			.iter()
			.merge_join_by(sample.mole_ids.iter(), |a, b| a.cmp(b))
		{
			match e {
				Both(_, _) => {
					if (unsafe {
						self.moles.get_unchecked(our_idx) - sample.moles.get_unchecked(sample_idx)
					})
					.abs() > min_delta
					{
						return true;
					}
					our_idx += 1;
					sample_idx += 1;
				}
				Left(_) => {
					if unsafe { *self.moles.get_unchecked(our_idx) } > min_delta {
						return true;
					}
					our_idx += 1;
				}
				Right(_) => {
					if unsafe { *sample.moles.get_unchecked(sample_idx) } > min_delta {
						return true;
					}
					sample_idx += 1;
				}
			}
		}
		self.total_moles() > min_delta
			&& (self.temperature - sample.temperature).abs() > MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND
	}
	/// Clears the moles from the gas.
	pub fn clear(&mut self) {
		if !self.immutable {
			self.mole_ids.clear();
			self.moles.clear();
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
		}
	}
	/// Checks if the proc can react with any reactions.
	pub fn can_react(&self) -> bool {
		reactions().iter().any(|r| r.check_conditions(self))
	}
	/// Gets all of the reactions this mix should do.
	pub fn all_reactable(&self) -> Vec<&'static Reaction> {
		reactions()
			.iter()
			.filter(|r| r.check_conditions(self))
			.collect()
	}
	/// Adds heat directly to the gas mixture, in joules (probably).
	pub fn adjust_heat(&mut self, heat: f32) {
		let cap = self.heat_capacity();
		self.set_temperature(((cap * self.temperature) + heat) / cap);
	}
	/// Returns true if there's a visible gas in this mix.
	pub fn is_visible(&self) -> bool {
		self.enumerate().any(|(&i, gas)| {
			if let Some(amt) = gas_visibility(i as usize) {
				gas >= &amt
			} else {
				false
			}
		})
	}
	/// A hashed representation of the visibility of a gas, so that it only needs to update vis when actually changed.
	pub fn visibility_hash(&self) -> u64 {
		use std::hash::Hasher;
		let mut hasher = std::collections::hash_map::DefaultHasher::new();
		for (&i, gas) in self.enumerate() {
			if let Some(amt) = gas_visibility(i as usize) {
				if gas >= &amt {
					hasher.write(&[
						i,
						FACTOR_GAS_VISIBLE_MAX.min((gas / MOLES_GAS_VISIBLE_STEP).ceil()) as u8,
					]);
				}
			}
		}
		hasher.finish()
	}
	// Removes all zeroes from the gas mixture.
	fn garbage_collect(&mut self) {
		// this is absolutely just a copy job of the source for the rust standard library's retain
		let mut del = 0;
		let len = self.moles.len();
		{
			for idx in 0..len {
				let amt = unsafe { *self.moles.get_unchecked(idx) };
				if !amt.is_normal() || amt < GAS_MIN_MOLES {
					del += 1;
				} else if del > 0 {
					self.moles.swap(idx - del, idx);
					self.mole_ids.swap(idx - del, idx);
				}
			}
		}
		if del > 0 {
			self.moles.truncate(len - del);
			self.mole_ids.truncate(len - del);
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
		assert_eq!(removed.compare(&new, MINIMUM_MOLES_DELTA_TO_MOVE), false);
		assert_eq!(removed.get_moles(0), 11.0);
		assert_eq!(removed.get_moles(1), 41.0);
		removed.mark_immutable();
		let new_two = removed.remove_ratio(0.5);
		assert_eq!(removed.compare(&new_two, MINIMUM_MOLES_DELTA_TO_MOVE), true);
		assert_eq!(removed.get_moles(0), 11.0);
		assert_eq!(removed.get_moles(1), 41.0);
		assert_eq!(new_two.get_moles(0), 5.5);
	}
}
