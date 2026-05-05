use std::time::Duration;

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use miette::Result;
use tracing::warn;

use crate::actions::iti::{
	display::{Canvas, Widget},
	samplers::temperature::sample,
};

const GREEN: Rgb565 = Rgb565::new(0, 255, 0);
const YELLOW: Rgb565 = Rgb565::new(255, 255, 0);
const RED: Rgb565 = Rgb565::new(255, 0, 0);

pub struct TemperatureWidget {
	area: Rectangle,
	last: Option<String>,
}

impl TemperatureWidget {
	pub fn new(area: Rectangle) -> Self {
		Self { area, last: None }
	}
}

impl Widget for TemperatureWidget {
	fn name(&self) -> &'static str {
		"temperature"
	}

	fn interval(&self) -> Duration {
		Duration::from_secs(10)
	}

	async fn tick(&mut self, canvas: &mut Canvas<'_>) -> Result<()> {
		let temp = match sample() {
			Ok(t) => t,
			Err(err) => {
				warn!(?err, "vcgencmd failed");
				return Ok(());
			}
		};
		let text = format!("{temp:>5.1}C");
		let stroke = if temp < 60.0 {
			GREEN
		} else if temp > 80.0 {
			RED
		} else {
			YELLOW
		};

		if self.last.as_deref() == Some(text.as_str()) {
			return Ok(());
		}

		canvas.clear_area(self.area)?;
		let baseline = Point::new(self.area.top_left.x, self.area.top_left.y + 16);
		canvas.text(baseline, &text, stroke)?;
		self.last = Some(text);
		Ok(())
	}
}
