use std::{collections::VecDeque, iter::repeat, time::Duration};

use clap::Parser;
use miette::Result;
use sysinfo::System;

use crate::actions::{
	iti::lcd::{
		json::{Item, Screen},
		send,
	},
	Context,
};

/// Display CPU and memory usage as spark lines on the LCD.
#[derive(Debug, Clone, Parser)]
pub struct SparksArgs {
	/// Y position of the gauges.
	#[arg(long, default_value = "30")]
	pub y: i32,

	/// Height of the gauges.
	#[arg(long, default_value = "27")]
	pub h: u32,

	/// Refresh interval.
	#[arg(long, default_value = "10s")]
	pub interval: humantime::Duration,

	/// ZMQ socket to use for screen updates.
	#[arg(default_value = "tcp://[::1]:2009")]
	pub zmq_socket: String,
}

const FG_CPU: [u8; 3] = [245, 0, 0];
const FG_MEM: [u8; 3] = [0, 0, 242];
const BG: [u8; 3] = [0, 0, 0];
const MIN_X: i32 = 10;
const MAX_X: i32 = 270;
const GAP: i32 = 10;

const OUTER_WIDTH: u32 = ((MAX_X - MIN_X - GAP) as u32) / 2;
const INNER_WIDTH: u32 = OUTER_WIDTH - 2;

pub async fn run(ctx: Context<SparksArgs>) -> Result<()> {
	let mut sys = System::new();
	sys.refresh_cpu_usage();
	sys.refresh_memory();

	let mut cpu = VecDeque::new();
	let mut mem = VecDeque::new();

	let mut interval: Duration = ctx.args_top.interval.into();
	if interval < sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
		interval = sysinfo::MINIMUM_CPU_UPDATE_INTERVAL;
	}

	loop {
		tokio::time::sleep(interval).await;
		sys.refresh_cpu_usage();
		sys.refresh_memory();

		let mut cpu_sum = 0.0;
		let mut cpu_count = 0.0;
		for cpu in sys.cpus() {
			cpu_sum += cpu.cpu_usage() / 100.0;
			cpu_count += 1.0;
		}
		cpu.push_front(cpu_sum / cpu_count);
		cpu.truncate(INNER_WIDTH as _);

		mem.push_front(sys.used_memory() as f32 / sys.total_memory() as f32);
		mem.truncate(INNER_WIDTH as _);

		render(
			&ctx.args_top,
			cpu.iter().rev().copied(),
			mem.iter().rev().copied(),
		)?;
	}
}

pub fn render(
	args: &SparksArgs,
	cpu: impl ExactSizeIterator<Item = f32>,
	mem: impl ExactSizeIterator<Item = f32>,
) -> Result<()> {
	let SparksArgs {
		y, h, zmq_socket, ..
	} = args;
	let y = *y;
	let h = *h;

	let inner_height = h - 2;
	let inner_y = y + 1;

	let mut items = vec![
		Item {
			x: MIN_X,
			y,
			fill: Some(FG_CPU),
			width: Some(OUTER_WIDTH),
			height: Some(h),
			..Default::default()
		},
		Item {
			x: MIN_X + 1,
			y: inner_y,
			fill: Some(BG),
			width: Some(INNER_WIDTH),
			height: Some(inner_height),
			..Default::default()
		},
		Item {
			x: MIN_X + (OUTER_WIDTH as i32) + GAP,
			y,
			fill: Some(FG_MEM),
			width: Some(OUTER_WIDTH),
			height: Some(h),
			..Default::default()
		},
		Item {
			x: MIN_X + (OUTER_WIDTH as i32) + GAP + 1,
			y: inner_y,
			fill: Some(BG),
			width: Some(INNER_WIDTH),
			height: Some(inner_height),
			..Default::default()
		},
	];

	items.extend(spark_line(
		cpu,
		MIN_X + 1,
		inner_y,
		inner_height as i32,
		FG_CPU,
	));
	items.extend(spark_line(
		mem,
		MIN_X + (OUTER_WIDTH as i32) + GAP + 1,
		inner_y,
		inner_height as i32,
		FG_MEM,
	));

	send(&zmq_socket, Screen::Layout(items))?;

	Ok(())
}

fn spark_line<'a>(
	data: impl ExactSizeIterator<Item = f32> + 'a,
	min_x: i32,
	min_y: i32,
	height: i32,
	colour: [u8; 3],
) -> impl Iterator<Item = Item> + 'a {
	repeat(0.0)
		.take((INNER_WIDTH as usize).saturating_sub(data.len()))
		.chain(data)
		.map(move |v| {
			let v = v.clamp(0.0, 1.0);
			let h = height as f32;
			((1.0 - v) * h).round() as i32
		})
		.enumerate()
		.map(move |(x, y)| Item {
			x: min_x + x as i32,
			y: min_y + y,
			width: Some(1),
			height: Some((height.saturating_sub(y).max(1) * 2 - 1) as u32),
			fill: Some(colour),
			..Default::default()
		})
}
