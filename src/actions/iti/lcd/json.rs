use embedded_graphics::{
	mono_font::{ascii::FONT_10X20, MonoTextStyle},
	pixelcolor::Rgb565,
	prelude::*,
	primitives::{PrimitiveStyle, Rectangle},
	text::Text,
};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Screen {
	pub background: [u8; 3],
	#[serde(default)]
	pub items: Vec<Item>,
	#[serde(default)]
	pub off: bool,
	#[serde(default)]
	pub clear: bool,
}

impl Drawable for Screen {
	type Color = Rgb565;
	type Output = ();

	fn draw<D>(&self, target: &mut D) -> Result<Self::Output, D::Error>
	where
		D: DrawTarget<Color = Self::Color>,
	{
		if self.clear {
			target.clear(Rgb565::new(
				self.background[0],
				self.background[1],
				self.background[2],
			))?;
		}

		for item in &self.items {
			item.draw(target)?;
		}

		Ok(())
	}
}

#[derive(Clone, Debug, Deserialize)]
pub struct Item {
	pub x: i32,
	pub y: i32,
	pub width: Option<u32>,
	pub height: Option<u32>,
	pub fill: Option<[u8; 3]>,
	pub stroke: Option<[u8; 3]>,
	pub text: Option<String>,
}

impl Drawable for Item {
	type Color = Rgb565;
	type Output = ();

	fn draw<D>(&self, target: &mut D) -> Result<Self::Output, D::Error>
	where
		D: DrawTarget<Color = Self::Color>,
	{
		if let (Some(width), Some(height), Some(colour)) = (self.width, self.height, self.fill) {
			Rectangle::new(Point::new(self.x, self.y), Size::new(width, height))
				.into_styled(PrimitiveStyle::with_fill(Rgb565::new(
					colour[0], colour[1], colour[2],
				)))
				.draw(target)?;
		}

		if let (Some(text), Some(stroke)) = (self.text.as_deref(), self.stroke) {
			Text::new(
				text,
				Point::new(self.x, self.y),
				MonoTextStyle::new(&FONT_10X20, Rgb565::new(stroke[0], stroke[1], stroke[2])),
			)
			.draw(target)?;
		}

		Ok(())
	}
}
