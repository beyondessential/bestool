use std::time::Duration;

use chrono::Local;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use miette::Result;

use crate::actions::iti::display::{Canvas, Widget};

// Matches the colour passed by the original `iti-localtime` script. (The 8-bit channels are
// masked to 5/6/5 bits by `Rgb565::new`, yielding a soft low-saturation tone — keeping it
// verbatim avoids visual regression on the panel.)
const STROKE: Rgb565 = Rgb565::new(235, 225, 205);

pub struct ClockWidget {
	area: Rectangle,
	last: Option<String>,
}

impl ClockWidget {
	pub fn new(area: Rectangle) -> Self {
		Self { area, last: None }
	}
}

impl Widget for ClockWidget {
	fn name(&self) -> &'static str {
		"clock"
	}

	fn interval(&self) -> Duration {
		Duration::from_secs(10)
	}

	async fn tick(&mut self, canvas: &mut Canvas<'_>) -> Result<()> {
		let now = Local::now();
		let text = format!("{} {}", now.format("%Y-%m-%d"), now.format("%H:%M"));
		if self.last.as_deref() == Some(text.as_str()) {
			return Ok(());
		}

		canvas.clear_area(self.area)?;
		// Text baseline sits 16px below the area's top edge to vertically centre FONT_10X20.
		let baseline = Point::new(self.area.top_left.x, self.area.top_left.y + 16);
		canvas.text(baseline, &text, STROKE)?;
		self.last = Some(text);
		Ok(())
	}
}
