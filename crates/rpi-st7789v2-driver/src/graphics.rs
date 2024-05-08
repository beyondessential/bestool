use embedded_graphics::{
	draw_target::DrawTarget,
	geometry::{Dimensions, Point, Size},
	pixelcolor::{
		raw::{RawData, RawU16},
		Rgb565,
	},
	primitives::Rectangle,
	Pixel,
};
use itertools::Itertools;
use tracing::instrument;

use super::{
	commands::*,
	error::{Error, Result},
	simple::*,
};

impl crate::Driver {
	/// Set the area of the screen to draw to.
	#[instrument(level = "trace", skip(self))]
	pub(crate) fn set_window(&mut self, start: (u16, u16), end: (u16, u16)) -> Result<()> {
		if (start.0 > end.0) || (start.1 > end.1) {
			return Err(Error::Io(std::io::Error::new(
				std::io::ErrorKind::InvalidInput,
				"window start must be equal or before end",
			)));
		}

		if (self.width < end.0) || (self.height < end.1) {
			return Err(Error::Io(std::io::Error::new(
				std::io::ErrorKind::InvalidInput,
				"window exceeds screen size",
			)));
		}

		// if this doesn't work, let's have another look at the c driver code
		self.command(Command::ColumnAddressSet)?;
		self.write_data(&(self.x_offset + start.0).to_be_bytes())?;
		self.write_data(&(self.x_offset + end.0).to_be_bytes())?;
		self.command(Command::RowAddressSet)?;
		self.write_data(&(self.y_offset + start.1).to_be_bytes())?;
		self.write_data(&(self.y_offset + end.1).to_be_bytes())?;

		Ok(())
	}

	/// Write an image to the screen, buffered.
	#[instrument(level = "trace", skip(self, image))]
	pub fn print(&mut self, origin: (u16, u16), image: &SimpleImage) -> Result<()> {
		self.set_window(
			origin,
			(
				origin.0.saturating_add(image.width),
				origin.1.saturating_add(image.height),
			),
		)?;

		self.command(Command::MemoryWrite)?;
		self.clear_buffer();
		for line in &image.data().chunks((image.width as usize) * 2) {
			self.write_data_buffered(&line.collect::<Vec<u8>>())?;
		}
		self.flush_buffer()?;
		self.command(Command::Nop)?;
		Ok(())
	}

	/// Write a pixel to the screen, unbuffered.
	#[instrument(level = "trace", skip(self))]
	pub fn pixel(&mut self, x: u16, y: u16, colour: Rgb565) -> Result<()> {
		if x >= self.width || y >= self.height {
			return Err(Error::Io(std::io::Error::new(
				std::io::ErrorKind::InvalidInput,
				"pixel out of bounds",
			)));
		}

		self.set_window((x, y), (x, y))?;
		self.command(Command::MemoryWrite)?;
		self.write_data(&RawU16::from(colour).into_inner().to_be_bytes())?;
		self.command(Command::Nop)?;
		Ok(())
	}
}

impl Dimensions for crate::Driver {
	fn bounding_box(&self) -> Rectangle {
		Rectangle::new(
			Point::new(0, 0),
			Size::new(self.width.into(), self.height.into()),
		)
	}
}

impl DrawTarget for crate::Driver {
	type Color = Rgb565;
	type Error = Error;

	fn draw_iter<I>(&mut self, pixels: I) -> std::result::Result<(), Self::Error>
	where
		I: IntoIterator<Item = Pixel<Self::Color>>,
	{
		for Pixel(coord, color) in pixels.into_iter() {
			let Ok(x) = u16::try_from(coord.x) else {
				continue;
			};
			let Ok(y) = u16::try_from(coord.y) else {
				continue;
			};

			if x >= self.width || y >= self.height {
				continue;
			}

			self.pixel(x, y, color)?;
		}

		Ok(())
	}

	#[instrument(level = "trace", skip(self, pixels))]
	fn fill_contiguous<I>(
		&mut self,
		area: &Rectangle,
		pixels: I,
	) -> std::result::Result<(), Self::Error>
	where
		I: IntoIterator<Item = Self::Color>,
	{
		let Ok(x) = u16::try_from(area.top_left.x) else {
			return Ok(());
		};
		let Ok(y) = u16::try_from(area.top_left.y) else {
			return Ok(());
		};
		let Ok(w) = u16::try_from(area.size.width) else {
			return Ok(());
		};
		let Ok(h) = u16::try_from(area.size.height) else {
			return Ok(());
		};

		let mut image = self.image();
		image.pixels = pixels
			.into_iter()
			.map(|c| RawU16::from(c).into_inner())
			.collect();
		image.resize(w, h);

		self.print((x, y), &image)
	}

	#[instrument(level = "trace", skip(self))]
	fn fill_solid(
		&mut self,
		area: &Rectangle,
		color: Self::Color,
	) -> std::result::Result<(), Self::Error> {
		let Ok(x) = u16::try_from(area.top_left.x) else {
			return Ok(());
		};
		let Ok(y) = u16::try_from(area.top_left.y) else {
			return Ok(());
		};
		let Ok(w) = u16::try_from(area.size.width) else {
			return Ok(());
		};
		let Ok(h) = u16::try_from(area.size.height) else {
			return Ok(());
		};

		let mut image = self.image();
		image.resize(w, h);
		image.solid(color);
		self.print((x, y), &image)
	}
}
