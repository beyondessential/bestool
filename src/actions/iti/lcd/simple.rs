use embedded_graphics::pixelcolor::{raw::{RawData, RawU16}, Rgb565};

/// Simple image buffer.
///
/// For more complex graphics, use the `embedded_graphics` crate.
#[derive(Debug, Clone)]
pub struct SimpleImage {
	pub(crate) width: u16, // readonly
	pub(crate) height: u16, // readonly
	pixels: Vec<u16>,
}

impl SimpleImage {
	pub(crate) fn new(width: u16, height: u16) -> Self {
		Self {
			width,
			height,
			pixels: vec![0; width as usize * height as usize],
		}
	}

	pub fn resize(&mut self, width: u16, height: u16) {
		self.width = width;
		self.height = height;
		self.pixels.resize(width as usize * height as usize, 0);
	}

	pub fn solid(&mut self, colour: Rgb565) {
		self.pixels.fill(RawU16::from(colour).into_inner());
	}

	pub(crate) fn data(&self) -> impl Iterator<Item=u8> + '_ {
		self.pixels.iter().flat_map(|p| p.to_be_bytes())
	}
}
