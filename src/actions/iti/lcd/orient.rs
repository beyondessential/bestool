use embedded_graphics::{
	draw_target::DrawTarget, geometry::Dimensions, pixelcolor::Rgb565, primitives::Rectangle, Pixel,
};
use gfx_xtra::draw_target::{DrawTargetExt2, RotateAngle, Rotated};

use super::io::{LcdIo, LcdIoError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Angle {
	Degrees0,
	Degrees90,
	Degrees180,
	Degrees270,
}

pub enum Oriented<'a> {
	Normal(&'a mut LcdIo),
	Rotated(Rotated<'a, LcdIo>),
}

impl<'a> Oriented<'a> {
	pub fn new(lcd: &'a mut LcdIo, angle: Angle) -> Self {
		match angle {
			Angle::Degrees0 => Self::Normal(lcd),
			Angle::Degrees90 => {
				lcd.x_offset = 0;
				lcd.y_offset = 0;
				Self::Rotated(lcd.rotated(RotateAngle::Degrees90))
			}
			Angle::Degrees180 => {
				lcd.x_offset = 0;
				lcd.y_offset = 0;
				Self::Rotated(lcd.rotated(RotateAngle::Degrees180))
			}
			Angle::Degrees270 => {
				lcd.x_offset = 0;
				lcd.y_offset = 0;
				Self::Rotated(lcd.rotated(RotateAngle::Degrees270))
			}
		}
	}
}

impl Dimensions for Oriented<'_> {
	fn bounding_box(&self) -> Rectangle {
		match self {
			Oriented::Normal(lcd) => lcd.bounding_box(),
			Oriented::Rotated(rotated) => rotated.bounding_box(),
		}
	}
}

impl DrawTarget for Oriented<'_> {
	type Color = Rgb565;
	type Error = LcdIoError;

	fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
	where
		I: IntoIterator<Item = Pixel<Self::Color>>,
	{
		match self {
			Oriented::Normal(lcd) => lcd.draw_iter(pixels),
			Oriented::Rotated(rotated) => rotated.draw_iter(pixels),
		}
	}

	fn fill_contiguous<I>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error>
	where
		I: IntoIterator<Item = Self::Color>,
	{
		match self {
			Oriented::Normal(lcd) => lcd.fill_contiguous(area, colors),
			Oriented::Rotated(rotated) => rotated.fill_contiguous(area, colors),
		}
	}

	fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
		match self {
			Oriented::Normal(lcd) => lcd.fill_solid(area, color),
			Oriented::Rotated(rotated) => rotated.fill_solid(area, color),
		}
	}

	fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
		match self {
			Oriented::Normal(lcd) => lcd.clear(color),
			Oriented::Rotated(rotated) => rotated.clear(color),
		}
	}
}
