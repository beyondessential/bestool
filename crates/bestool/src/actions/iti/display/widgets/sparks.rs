use std::{collections::VecDeque, time::Duration};

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use miette::Result;
use sysinfo::System;

use crate::actions::iti::display::{Canvas, Widget};

const FG_CPU: Rgb565 = Rgb565::new(245, 0, 0);
const FG_MEM: Rgb565 = Rgb565::new(0, 0, 242);
const BG: Rgb565 = Rgb565::new(0, 0, 0);
const GAP: i32 = 10;

pub struct SparksWidget {
	area: Rectangle,
	system: System,
	cpu_history: VecDeque<f32>,
	mem_history: VecDeque<f32>,
	first_tick: bool,
}

impl SparksWidget {
	pub fn new(area: Rectangle) -> Self {
		let mut system = System::new();
		system.refresh_cpu_usage();
		system.refresh_memory();
		Self {
			area,
			system,
			cpu_history: VecDeque::new(),
			mem_history: VecDeque::new(),
			first_tick: true,
		}
	}

	fn outer_width(&self) -> u32 {
		(self.area.size.width.saturating_sub(GAP as u32)) / 2
	}

	fn inner_width(&self) -> u32 {
		self.outer_width().saturating_sub(2)
	}
}

impl Widget for SparksWidget {
	fn name(&self) -> &'static str {
		"sparks"
	}

	fn interval(&self) -> Duration {
		Duration::from_secs(10).max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
	}

	async fn tick(&mut self, canvas: &mut Canvas<'_>) -> Result<()> {
		self.system.refresh_cpu_usage();
		self.system.refresh_memory();

		let cpu_avg = if self.system.cpus().is_empty() {
			0.0
		} else {
			let sum: f32 = self.system.cpus().iter().map(|c| c.cpu_usage() / 100.0).sum();
			sum / self.system.cpus().len() as f32
		};
		let mem_ratio = self.system.used_memory() as f32 / self.system.total_memory().max(1) as f32;

		let inner = self.inner_width() as usize;
		self.cpu_history.push_front(cpu_avg);
		self.cpu_history.truncate(inner);
		self.mem_history.push_front(mem_ratio);
		self.mem_history.truncate(inner);

		// Background frames + inner clears (only on the first tick — afterwards we just refresh
		// the spark columns themselves, which fully overwrite the inner area).
		if self.first_tick {
			let outer = self.outer_width();
			let h = self.area.size.height;
			let inner_h = h.saturating_sub(2);
			let left = self.area.top_left;
			let right = Point::new(
				self.area.top_left.x + outer as i32 + GAP,
				self.area.top_left.y,
			);

			canvas.fill(Rectangle::new(left, Size::new(outer, h)), FG_CPU)?;
			canvas.fill(
				Rectangle::new(left + Point::new(1, 1), Size::new(self.inner_width(), inner_h)),
				BG,
			)?;
			canvas.fill(Rectangle::new(right, Size::new(outer, h)), FG_MEM)?;
			canvas.fill(
				Rectangle::new(right + Point::new(1, 1), Size::new(self.inner_width(), inner_h)),
				BG,
			)?;

			self.first_tick = false;
		}

		let inner_height = self.area.size.height.saturating_sub(2) as i32;
		let inner_y = self.area.top_left.y + 1;

		draw_spark(
			canvas,
			self.cpu_history.iter().rev().copied(),
			self.area.top_left.x + 1,
			inner_y,
			inner_height,
			self.inner_width() as i32,
			FG_CPU,
		)?;
		draw_spark(
			canvas,
			self.mem_history.iter().rev().copied(),
			self.area.top_left.x + self.outer_width() as i32 + GAP + 1,
			inner_y,
			inner_height,
			self.inner_width() as i32,
			FG_MEM,
		)?;
		Ok(())
	}
}

fn draw_spark(
	canvas: &mut Canvas<'_>,
	data: impl ExactSizeIterator<Item = f32>,
	min_x: i32,
	min_y: i32,
	height: i32,
	width: i32,
	colour: Rgb565,
) -> Result<()> {
	let len = data.len();
	let pad = (width as usize).saturating_sub(len);
	let h = height as f32;

	// Clear the inner column band first so the previous spark doesn't bleed through.
	canvas.fill(
		Rectangle::new(Point::new(min_x, min_y), Size::new(width as u32, height as u32)),
		BG,
	)?;

	for (i, v) in std::iter::repeat_n(0.0, pad).chain(data).enumerate() {
		let v = v.clamp(0.0, 1.0);
		let y = ((1.0 - v) * h).round() as i32;
		let bar_h = (height.saturating_sub(y).max(1) * 2 - 1).max(1) as u32;
		canvas.fill(
			Rectangle::new(Point::new(min_x + i as i32, min_y + y), Size::new(1, bar_h)),
			colour,
		)?;
	}
	Ok(())
}
