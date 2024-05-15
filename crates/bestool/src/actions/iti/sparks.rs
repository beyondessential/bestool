use std::iter::repeat;

use clap::Parser;
use miette::Result;

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
	let SparksArgs { y, h, zmq_socket } = ctx.args_top;

	let cpu = vec![
		0.1, 0.2, 0.3, 0.10, 0.2, 0.40, 0.2, 0.30, 0.1, 0.2, 0.3, 0.50, 0.55, 0.55, 0.55, 0.85,
		0.88, 0.90, 0.90, 0.90, 0.60, 0.30, 0.20, 0.20, 0.22, 0.21, 0.15, 0.16, 0.5, 0.2,
	];
	let mem = vec![
		0.1, 0.2, 0.3, 0.10, 0.2, 0.40, 0.2, 0.30, 0.1, 0.2, 0.3, 0.50, 0.55, 0.55, 0.55, 0.85,
		0.88, 0.90, 0.90, 0.90, 0.60, 0.30, 0.20, 0.20, 0.22, 0.21, 0.15, 0.16, 0.5, 0.2, 1.0,
	];

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
		&cpu,
		MIN_X + 1,
		inner_y,
		inner_height as i32,
		FG_CPU,
	));
	items.extend(spark_line(
		&mem,
		MIN_X + (OUTER_WIDTH as i32) + GAP + 1,
		inner_y,
		inner_height as i32,
		FG_MEM,
	));

	send(&zmq_socket, Screen::Layout(items))?;

	Ok(())
}

fn spark_line(
	data: &[f32],
	min_x: i32,
	min_y: i32,
	height: i32,
	colour: [u8; 3],
) -> impl Iterator<Item = Item> + '_ {
	repeat(0.0)
		.take((INNER_WIDTH as usize).saturating_sub(data.len()))
		.chain(data.iter().copied())
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
