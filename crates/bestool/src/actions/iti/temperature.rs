use clap::Parser;
use miette::Result;
use tokio::time::sleep;

use crate::actions::{
	Context,
	iti::{ItiArgs, samplers::temperature::sample},
};
#[cfg(feature = "iti-lcd")]
use crate::actions::iti::lcd::{
	json::{Item, Screen},
	send,
};

/// Get core temperature from the Raspberry Pi.
#[derive(Debug, Clone, Parser)]
pub struct TemperatureArgs {
	/// Output in JSON format.
	#[arg(long)]
	pub json: bool,

	/// Update screen with temperature.
	///
	/// Argument is the Y position of the temperature display. The X position is always 240 (right edge).
	#[cfg(feature = "iti-lcd")]
	#[arg(long)]
	pub update_screen: Option<i32>,

	/// ZMQ socket to use for screen updates.
	#[cfg(feature = "iti-lcd")]
	#[arg(default_value = "tcp://[::1]:2009")]
	pub zmq_socket: String,

	/// Keep updating at an interval.
	///
	/// Syntax is a number followed by a unit, such as "5s" or "1m".
	#[arg(long)]
	pub watch: Option<humantime::Duration>,
}

pub async fn run(ctx: Context<ItiArgs, TemperatureArgs>) -> Result<()> {
	if let Some(n) = ctx.args_sub.watch {
		loop {
			once(&ctx.args_sub)?;
			sleep(*n).await;
		}
	} else {
		once(&ctx.args_sub)
	}
}

fn once(args: &TemperatureArgs) -> Result<()> {
	let temperature = sample()?;
	if args.json {
		println!("{}", serde_json::json!({ "temperature": temperature }));
	} else {
		println!("{:.1}°C", temperature);
	}

	#[cfg(feature = "iti-lcd")]
	if let Some(y) = args.update_screen {
		update_screen(args, y, temperature);
	}

	Ok(())
}

#[cfg(feature = "iti-lcd")]
fn update_screen(args: &TemperatureArgs, y: i32, temperature: f32) {
	const GREEN: [u8; 3] = [0, 255, 0];
	const RED: [u8; 3] = [255, 0, 0];
	const BLACK: [u8; 3] = [0, 0, 0];
	const YELLOW: [u8; 3] = [255, 255, 0];

	let stroke = if temperature < 60.0 {
		GREEN
	} else if temperature > 80.0 {
		RED
	} else {
		YELLOW
	};

	let _ = send(
		&args.zmq_socket,
		Screen::Layout(vec![
			Item {
				x: 218,
				y: y - 16,
				width: Some(62),
				height: Some(20),
				fill: Some(BLACK),
				..Default::default()
			},
			Item {
				x: 220,
				y,
				stroke: Some(stroke),
				text: Some(format!("{temperature:>5.1}C")),
				..Default::default()
			},
		]),
	);
}
