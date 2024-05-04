use std::io::Read;

use bitvec::vec::BitVec;

use super::chip::Chip;

#[derive(Clone, Debug)]
pub struct Pixels {
	pub width: u16,
	pub height: u16,
	pub data: BitVec,
}

impl Pixels {
	pub fn new(width: u16, height: u16) -> Self {
		let mut data = BitVec::with_capacity((width * height) as usize);
		data.resize(data.capacity(), false);

		Self {
			width,
			height,
			data,
		}
	}

	pub fn new_for(chip: Chip) -> Self {
		Self::new(chip.width(), chip.height())
	}

	/// Set a pixel.
	///
	/// What this does depends on the buffer's usage. For black/white pixels, it sets the pixel to
	/// white (true) or black (false). For red/transparent pixels, it sets whether the pixel is red
	/// (true) or transparent (false). Transparent pixels display as the black/white value.
	///
	/// # Panics
	///
	/// Panics if the coordinates are out of bounds.
	pub fn set(&mut self, x: u16, y: u16, value: bool) {
		let idx = (y * self.width + x) as usize;
		self.data.set(idx, value);
	}

	/// Get a pixel.
	///
	/// # Panics
	///
	/// Panics if the coordinates are out of bounds.
	#[allow(dead_code)] // provided for completeness, not actually used here
	pub fn get(&self, x: u16, y: u16) -> bool {
		let idx = (y * self.width + x) as usize;
		*self.data.get(idx).as_deref().unwrap()
	}

	/// Fill with a single value.
	pub fn fill(&mut self, value: bool) {
		self.data.fill(value);
	}

	/// Get the underlying data as a [`Read`]er.
	pub fn as_reader(&mut self) -> impl Read + '_ {
		&mut self.data
	}
}
