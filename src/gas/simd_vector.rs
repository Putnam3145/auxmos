pub use packed_simd_2::*;

pub type SimdGasVec = f32x4; // 8 got reasonable performance but this seems to work better, + it's more portable i guess

use std::ops::*;

#[derive(Clone)]
pub struct FlatSimdVec {
	internal: Vec<SimdGasVec>,
	len: usize,
}

impl FlatSimdVec {
	const LANE_SIZE: usize = SimdGasVec::lanes();
	pub fn new() -> Self {
		FlatSimdVec {
			internal: Vec::new(),
			len: 0,
		}
	}
	pub fn from_vec(internal: Vec<SimdGasVec>) -> Self {
		let len = internal.len() * Self::LANE_SIZE;
		let mut res = FlatSimdVec { internal, len };
		res.garbage_collect();
		res
	}
	pub fn get(&self, idx: usize) -> Option<f32> {
		(idx < self.len).then(|| unsafe {
			self.internal
				.get_unchecked(idx / Self::LANE_SIZE)
				.extract_unchecked(idx % Self::LANE_SIZE)
		})
	}
	pub fn force_set(&mut self, idx: usize, n: f32) {
		self.len = (idx + 1).max(self.len);
		self.correct_size();
		unsafe { self.set_unchecked(idx, n) };
	}
	pub fn set(&mut self, idx: usize, n: f32) -> bool {
		self.correct_size();
		if idx < self.len {
			self.internal[idx / Self::LANE_SIZE] = self.internal.get(idx / Self::LANE_SIZE).unwrap().replace(idx % Self::LANE_SIZE, n);
			true
		} else {
			false
		}
	}
	pub unsafe fn set_unchecked(&mut self, idx: usize, n: f32) {
		let entry = self.internal.get_unchecked_mut(idx / Self::LANE_SIZE);
		*entry = entry.replace_unchecked(idx % Self::LANE_SIZE, n);
	}
	pub fn len(&self) -> usize {
		self.len
	}
	pub fn internal_len(&self) -> usize {
		self.internal.len()
	}
	pub fn expand(&mut self, size: usize) {
		if size > self.len {
			self.resize(size);
		}
	}
	fn correct_size(&mut self) {
		if self.len < self.internal.len() * Self::LANE_SIZE {
			self.garbage_collect();
		}
		while self.internal.len() * Self::LANE_SIZE < self.len {
			self.internal.push(SimdGasVec::splat(0.0));
		}
	}
	pub fn push(&mut self, val: f32) {
		let prev_len = self.len;
		self.len += 1;
		self.correct_size();
		self.set(prev_len, val);
	}
	pub fn resize(&mut self, size: usize) {
		self.len = size;
		self.correct_size();
	}
	pub fn sum(&self) -> f32 {
		self.internal.iter().sum::<SimdGasVec>().sum()
	}
	pub fn dot_product(&self, other: &Self) -> f32 {
		let mut acc = SimdGasVec::splat(0.0);
		for (a, b) in self.iter().copied().zip(other.iter().copied()) {
			acc = a.mul_adde(b, acc);
		}
		acc.sum()
	}
	pub fn truncate(&mut self) {
		self.len = self.len.min(self.internal.len() * Self::LANE_SIZE);
	}
	pub fn clear(&mut self) {
		self.internal.clear();
		self.len = 0;
	}
	pub fn garbage_collect(&mut self) {
		loop {
			if self.internal.len() <= 1
				|| self
					.internal
					.last()
					.map(|vector| vector.sum())
					.map_or(true, |s| {
						if s <= crate::GAS_MIN_MOLES || !s.is_normal() {
							self.internal.pop();
							false
						} else {
							true
						}
					}) {
				break;
			}
		}
		self.truncate();
	}
}

impl Deref for FlatSimdVec {
	type Target = Vec<SimdGasVec>;

	fn deref(&self) -> &Self::Target {
		&self.internal
	}
}

impl AddAssign<Self> for FlatSimdVec {
	fn add_assign(&mut self, other: Self) {
		for (i, entry) in other.iter().copied().enumerate() {
			if let Some(ours) = self.internal.get_mut(i) {
				*ours += entry;
			} else {
				self.internal.push(entry);
			}
		}
	}
}

impl Add<Self> for FlatSimdVec {
	type Output = Self;
	fn add(self, rhs: Self) -> Self {
		let mut ret = self.clone();
		ret += rhs;
		ret
	}
}

impl SubAssign<Self> for FlatSimdVec {
	fn sub_assign(&mut self, other: Self) {
		for (i, entry) in other.iter().copied().enumerate() {
			if let Some(ours) = self.internal.get_mut(i) {
				*ours -= entry;
			} else {
				self.internal.push(entry);
			}
		}
	}
}

impl Sub<Self> for FlatSimdVec {
	type Output = Self;
	fn sub(self, rhs: Self) -> Self {
		let mut ret = self.clone();
		ret -= rhs;
		ret
	}
}

impl MulAssign<Self> for FlatSimdVec {
	fn mul_assign(&mut self, other: Self) {
		for (i, entry) in other.iter().copied().enumerate() {
			if let Some(ours) = self.internal.get_mut(i) {
				*ours *= entry;
			} else {
				break;
			}
		}
	}
}

impl Mul<Self> for FlatSimdVec {
	type Output = Self;
	fn mul(self, rhs: Self) -> Self {
		let mut ret = self.clone();
		ret *= rhs;
		ret
	}
}

impl DivAssign<Self> for FlatSimdVec {
	fn div_assign(&mut self, other: Self) {
		for (i, entry) in other.iter().copied().enumerate() {
			if let Some(ours) = self.internal.get_mut(i) {
				*ours /= entry;
			} else {
				break;
			}
		}
	}
}

impl Div<Self> for FlatSimdVec {
	type Output = Self;
	fn div(self, rhs: Self) -> Self {
		let mut ret = self.clone();
		ret /= rhs;
		ret
	}
}

impl AddAssign<&Self> for FlatSimdVec {
	fn add_assign(&mut self, other: &Self) {
		for (i, entry) in other.iter().copied().enumerate() {
			if let Some(ours) = self.internal.get_mut(i) {
				*ours += entry;
			} else {
				self.internal.push(entry);
			}
		}
	}
}

impl Add<&Self> for FlatSimdVec {
	type Output = Self;
	fn add(self, rhs: &Self) -> Self {
		let mut ret = self.clone();
		ret += rhs;
		ret
	}
}

impl SubAssign<&Self> for FlatSimdVec {
	fn sub_assign(&mut self, other: &Self) {
		for (i, entry) in other.iter().copied().enumerate() {
			if let Some(ours) = self.internal.get_mut(i) {
				*ours -= entry;
			} else {
				self.internal.push(entry);
			}
		}
	}
}

impl Sub<&Self> for FlatSimdVec {
	type Output = Self;
	fn sub(self, rhs: &Self) -> Self {
		let mut ret = self.clone();
		ret -= rhs;
		ret
	}
}

impl MulAssign<&Self> for FlatSimdVec {
	fn mul_assign(&mut self, other: &Self) {
		for (i, entry) in other.iter().copied().enumerate() {
			if let Some(ours) = self.internal.get_mut(i) {
				*ours *= entry;
			} else {
				break;
			}
		}
	}
}

impl Mul<&Self> for FlatSimdVec {
	type Output = Self;
	fn mul(self, rhs: &Self) -> Self {
		let mut ret = self.clone();
		ret *= rhs;
		ret
	}
}

impl<'a, 'b> Mul<&'a FlatSimdVec> for &'b FlatSimdVec {
	type Output = FlatSimdVec;
	fn mul(self, rhs: &'a FlatSimdVec) -> FlatSimdVec {
		let mut ret = self.clone();
		ret *= rhs;
		ret
	}
}

impl DivAssign<&Self> for FlatSimdVec {
	fn div_assign(&mut self, other: &Self) {
		for (i, entry) in other.iter().copied().enumerate() {
			if let Some(ours) = self.internal.get_mut(i) {
				*ours /= entry;
			} else {
				break;
			}
		}
	}
}

impl Div<&Self> for FlatSimdVec {
	type Output = Self;
	fn div(self, rhs: &Self) -> Self {
		let mut ret = self.clone();
		ret /= rhs;
		ret
	}
}

pub trait RhsForFlatSimdVec {}

impl RhsForFlatSimdVec for f32 {}

impl RhsForFlatSimdVec for SimdGasVec {}

impl<T: RhsForFlatSimdVec + Copy> AddAssign<T> for FlatSimdVec
where
	SimdGasVec: std::ops::AddAssign<T>,
{
	fn add_assign(&mut self, other: T) {
		self.internal.iter_mut().for_each(|o| *o += other);
	}
}

impl<T: RhsForFlatSimdVec + Copy> Add<T> for FlatSimdVec
where
	SimdGasVec: std::ops::AddAssign<T>,
{
	type Output = Self;

	fn add(self, rhs: T) -> Self {
		let mut ret = self.clone();
		ret += rhs;
		ret
	}
}

impl<T: RhsForFlatSimdVec + Copy> SubAssign<T> for FlatSimdVec
where
	SimdGasVec: std::ops::SubAssign<T>,
{
	fn sub_assign(&mut self, other: T) {
		self.internal.iter_mut().for_each(|o| *o -= other);
	}
}

impl<T: RhsForFlatSimdVec + Copy> Sub<T> for FlatSimdVec
where
	SimdGasVec: std::ops::SubAssign<T>,
{
	type Output = Self;

	fn sub(self, rhs: T) -> Self {
		let mut ret = self.clone();
		ret -= rhs;
		ret
	}
}

impl<T: RhsForFlatSimdVec + Copy> MulAssign<T> for FlatSimdVec
where
	SimdGasVec: std::ops::MulAssign<T>,
{
	fn mul_assign(&mut self, other: T) {
		self.internal.iter_mut().for_each(|o| *o *= other);
	}
}

impl<T: RhsForFlatSimdVec + Copy> Mul<T> for FlatSimdVec
where
	SimdGasVec: std::ops::MulAssign<T>,
{
	type Output = Self;

	fn mul(self, rhs: T) -> Self {
		let mut ret = self.clone();
		ret *= rhs;
		ret
	}
}

impl<T: RhsForFlatSimdVec + Copy> DivAssign<T> for FlatSimdVec
where
	SimdGasVec: std::ops::DivAssign<T>,
{
	fn div_assign(&mut self, other: T) {
		self.internal.iter_mut().for_each(|o| *o /= other);
	}
}

impl<T: RhsForFlatSimdVec + Copy> Div<T> for FlatSimdVec
where
	SimdGasVec: std::ops::DivAssign<T>,
{
	type Output = Self;

	fn div(self, rhs: T) -> Self {
		let mut ret = self.clone();
		ret /= rhs;
		ret
	}
}
