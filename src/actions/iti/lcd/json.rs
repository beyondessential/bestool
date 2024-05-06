use embedded_graphics::{
	mono_font::{ascii::FONT_10X20, MonoTextStyle},
	pixelcolor::Rgb565,
	prelude::*,
	primitives::{PrimitiveStyle, Rectangle},
	text::Text,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Screen {
	Clear([u8; 3]),
	Light(bool),
	Layout(Vec<Item>),
}

impl Drawable for Screen {
	type Color = Rgb565;
	type Output = ();

	fn draw<D>(&self, target: &mut D) -> Result<Self::Output, D::Error>
	where
		D: DrawTarget<Color = Self::Color>,
	{
		use Screen::*;
		match self {
			Clear([r, g, b]) => target.clear(Rgb565::new(*r, *g, *b)),
			Layout(items) => {
				for item in items {
					item.draw(target)?;
				}
				Ok(())
			}
			Light(_) => unreachable!("Light is handled outside of the draw method"),
		}
	}
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
