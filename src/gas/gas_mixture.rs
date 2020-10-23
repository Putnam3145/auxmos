extern crate sprs;

use sprs::CsVec;

use super::constants::*;

use super::gas_specific_heats;

use super::total_num_gases;

#[derive(Clone)]
struct GasMixture {
	moles: CsVec<f32>,
	moles_archived: CsVec<f32>,
	temperature: f32,
	temperature_archived: f32,
	pub volume: f32,
	last_share: f32,
	pub min_heat_capacity: f32,
	immutable: bool,
}

impl GasMixture {
	/// Makes an empty gas mixture.
	fn new() -> Self {
		GasMixture {
			moles: CsVec::empty(total_num_gases() as usize),
			moles_archived: CsVec::empty(total_num_gases() as usize),
			temperature: 0.0,
			temperature_archived: 0.0,
			volume: 0.0,
			last_share: 0.0,
			min_heat_capacity: 0.0,
			immutable: false,
		}
	}
	/// Makes an empty gas mixture with the given volume.
	fn from_vol(vol: f32) -> Self {
		let mut ret: GasMixture = GasMixture::new();
		ret.volume = vol;
		ret
	}
	/// The heat capacity of the material. [joules?]/mole-kelvin.
	fn heat_capacity(&self) -> f32 {
		self.moles.dot_dense(gas_specific_heats()) // dot product is what we're doing here anyway
	}
	/// As heat_capacity, but using the archive, for consistency during processing.
	fn heat_capacity_archived(&self) -> f32 {
		self.moles_archived.dot_dense(gas_specific_heats())
	}
	/// The total mole count of the mixture. Moles.
	fn total_moles(&self) -> f32 {
		self.moles.data().iter().fold(0.0, |tot, gas| tot + gas)
	}
	/// Pressure. Kilopascals.
	fn return_pressure(&self) -> f32 {
		self.total_moles() * R_IDEAL_GAS_EQUATION * self.temperature / self.volume
	}
	/// Thermal energy. Joules?
	fn thermal_energy(&self) -> f32 {
		self.heat_capacity() * self.temperature
	}
	/// Sets the mole archive, for consistency during processing.
	fn archive(&mut self) {
		self.moles_archived = self.moles.clone();
		self.temperature_archived = self.temperature;
	}
	/// Merges one gas mixture into another.
	fn merge(&mut self, giver: &GasMixture) {
		if self.immutable {
			return;
		}
		let tot_energy = self.thermal_energy() + giver.thermal_energy();
		self.moles = &self.moles + &giver.moles;
		self.temperature = tot_energy / self.heat_capacity();
	}
	/// Returns a gas mixture that contains a given percentage of this mixture's moles; if this mix is mutable, also removes those moles from the original.
	fn remove_ratio(&mut self, mut ratio: f32) -> GasMixture {
		let mut removed = GasMixture::from_vol(self.volume);
		if ratio <= 0.0 {
			return removed;
		}
		if ratio >= 1.0 {
			ratio = 1.0;
		}
		removed.temperature = self.temperature;
		removed.moles = self.moles.clone();
		removed.moles *= ratio;
		if !self.immutable {
			self.moles = &self.moles + -(removed.moles.clone());
		}
		removed
	}
	/// As remove_ratio, but a raw number of moles instead of a ratio.
	fn remove(&mut self, amount: f32) -> GasMixture {
		self.remove_ratio(amount / self.total_moles())
	}
	/// Copies from a given gas mixture, if we're mutable.
	fn copy_from_mutable(&mut self, sample: &GasMixture) {
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
	 */
	fn share(&mut self, sharer: &mut GasMixture, atmos_adjacent_turfs: i32) -> f32 {
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

		let deltas = self.moles_archived.clone() + -sharer.moles_archived.clone();
		let heat_caps: Vec<f32> = deltas
			.iter()
			.map(|(idx, val)| val * gas_specific_heats()[idx])
			.collect();
		if !self.immutable {
			self.moles = self.moles.clone() + -deltas.clone();
		}
		if !sharer.immutable {
			sharer.moles = sharer.moles.clone() + deltas.clone();
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
		0.0
	}
	/**
	 * A very simple finite difference solution to the heat transfer equation.
	 * Works well enough for our purposes, though perhaps called less often
	 * than it ought to be while we're working in Rust.
	 */
	fn temperature_share(&mut self, sharer: &mut GasMixture, conduction_coefficient: f32) -> f32 {
		let temperature_delta = self.temperature_archived - sharer.temperature_archived;
		if temperature_delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
			let self_heat_capacity = self.heat_capacity_archived();
			let sharer_heat_capacity = sharer.heat_capacity_archived();

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
	fn temperature_share_non_gas(
		&mut self,
		conduction_coefficient: f32,
		sharer_temperature: f32,
		sharer_heat_capacity: f32,
	) -> f32 {
		let temperature_delta = self.temperature_archived - sharer_temperature;
		if temperature_delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER {
			let self_heat_capacity = self.heat_capacity_archived();

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
	fn compare(&self, sample: &GasMixture) -> i32 {
		for (i, (our_moles, their_moles)) in self
			.moles
			.data()
			.iter()
			.zip(sample.moles.data().iter())
			.enumerate()
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
	fn clear(&mut self) {
		if !self.immutable {
			self.moles.clear();
		}
	}
	/// Multiplies every gas molage with this value.
	fn multiply(&mut self, multiplier: f32) {
		if !self.immutable {
			self.moles *= multiplier;
		}
	}
}
