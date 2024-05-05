use embedded_graphics::{
	draw_target::DrawTarget,
	primitives::Rectangle,
	geometry::{Dimensions, Point, Size},
	pixelcolor::{
		raw::{RawData, RawU16},
		Rgb565,
	},
	Pixel,
};

/// Simple image buffer.
///
/// For more complex graphics, use the `embedded_graphics` crate.
#[derive(Debug, Clone)]
pub struct SimpleImage {
	pub(crate) width: u16,  // readonly
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

	pub(crate) fn index(&self, x: u16, y: u16) -> Result<usize, ()> {
		if x >= self.width || y >= self.height {
			Err(())
		} else {
			Ok((x + y * self.width) as usize)
		}
	}

	pub fn pixel(&mut self, x: u16, y: u16, colour: Rgb565) {
		if let Ok(index) = self.index(x, y) {
			self.pixels[index] = RawU16::from(colour).into_inner();
		}
	}

	pub(crate) fn data(&self) -> impl Iterator<Item = u8> + '_ {
		self.pixels.iter().flat_map(|p| p.to_be_bytes())
	}
}

impl Dimensions for SimpleImage {
	fn bounding_box(&self) -> Rectangle {
		Rectangle::new(Point::new(0, 0), Size::new(self.width.into(), self.height.into()))
	}
}

impl DrawTarget for SimpleImage {
	type Color = Rgb565;
	type Error = core::convert::Infallible;

	fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
	where
		I: IntoIterator<Item = Pixel<Self::Color>>,
	{
		for Pixel(coord, color) in pixels.into_iter() {
			let Ok(x) = u16::try_from(coord.x) else { continue };
			let Ok(y) = u16::try_from(coord.y) else { continue };
			self.pixel(x, y, color);
        }

		Ok(())
	}
}
