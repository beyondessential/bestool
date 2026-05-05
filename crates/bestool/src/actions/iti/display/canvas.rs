use embedded_graphics::{
	mono_font::{MonoTextStyle, ascii::FONT_10X20},
	pixelcolor::Rgb565,
	prelude::*,
	primitives::{PrimitiveStyle, Rectangle},
	text::Text,
};
use miette::Result;
use rpi_st7789v2_driver::Driver;

/// Drawing surface passed to widgets each tick. Wraps the LCD driver and exposes the small set
/// of primitives the widgets actually need (filled rectangles + monospace text).
pub struct Canvas<'d> {
	driver: &'d mut Driver,
}

impl<'d> Canvas<'d> {
	pub fn new(driver: &'d mut Driver) -> Self {
		Self { driver }
	}

	pub fn fill(&mut self, rect: Rectangle, color: Rgb565) -> Result<()> {
		rect.into_styled(PrimitiveStyle::with_fill(color))
			.draw(self.driver)?;
		Ok(())
	}

	pub fn text(&mut self, at: Point, s: &str, color: Rgb565) -> Result<()> {
		Text::new(s, at, MonoTextStyle::new(&FONT_10X20, color)).draw(self.driver)?;
		Ok(())
	}

	pub fn clear_area(&mut self, rect: Rectangle) -> Result<()> {
		self.fill(rect, Rgb565::BLACK)
	}
}
