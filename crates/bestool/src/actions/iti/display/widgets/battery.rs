use std::time::Duration;

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use folktime::duration::{Duration as Folktime, Style as Folkstyle};
use miette::Result;
use tracing::warn;

use crate::actions::iti::{
	display::{Canvas, Widget},
	samplers::battery::{BatteryEstimator, sample},
};

const TICK: Duration = Duration::from_secs(10);

const GREEN: Rgb565 = Rgb565::new(0, 255, 0);
const RED: Rgb565 = Rgb565::new(255, 0, 0);
const BLACK: Rgb565 = Rgb565::new(0, 0, 0);
const WHITE: Rgb565 = Rgb565::new(255, 255, 255);

pub struct BatteryWidget {
	area: Rectangle,
	estimator: BatteryEstimator,
	last: Option<String>,
}

impl BatteryWidget {
	pub fn new(area: Rectangle) -> Self {
		Self {
			area,
			estimator: BatteryEstimator::new(TICK),
			last: None,
		}
	}
}

impl Widget for BatteryWidget {
	fn name(&self) -> &'static str {
		"battery"
	}

	fn interval(&self) -> Duration {
		TICK
	}

	async fn tick(&mut self, canvas: &mut Canvas<'_>) -> Result<()> {
		let s = match sample() {
			Ok(s) => s,
			Err(err) => {
				warn!(?err, "battery sample failed");
				return Ok(());
			}
		};
		let est = self.estimator.observe(s.capacity);
		let charging = est.rate_per_second > 0.0;
		let stable = est.rate_per_second == 0.0;

		let (fill, stroke) = if charging {
			(GREEN, BLACK)
		} else if est.capacity <= 3.0 {
			(RED, WHITE)
		} else if est.capacity <= 15.0 {
			(BLACK, RED)
		} else {
			(BLACK, WHITE)
		};

		let pct_text = format!("{:>3.0}%", est.capacity);
		let side_text = if let Some(remaining) = est.time_remaining {
			Some(if est.rate_per_second < 0.0 {
				format!("{} left", folktime_pretty(remaining))
			} else {
				format!("full in {}", folktime_pretty(remaining))
			})
		} else if stable && est.capacity == 100.0 {
			Some("fully charged".into())
		} else {
			None
		};

		let composed = format!("{} | {}", pct_text, side_text.as_deref().unwrap_or(""));
		if self.last.as_deref() == Some(composed.as_str()) {
			return Ok(());
		}

		canvas.fill(self.area, fill)?;
		let pct_x = self.area.top_left.x + self.area.size.width as i32 - 40;
		let baseline_y = self.area.top_left.y + 16;
		canvas.text(Point::new(pct_x, baseline_y), &pct_text, stroke)?;
		if let Some(side) = side_text.as_deref() {
			canvas.text(
				Point::new(self.area.top_left.x + 2, baseline_y),
				side,
				stroke,
			)?;
		}
		self.last = Some(composed);
		Ok(())
	}
}

fn folktime_pretty(d: Duration) -> Folktime {
	Folktime(
		d,
		if d > Duration::from_secs(60 * 60) {
			Folkstyle::TwoUnitsWhole
		} else {
			Folkstyle::OneUnitWhole
		},
	)
}
