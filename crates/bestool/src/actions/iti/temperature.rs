use std::time::Duration;

use clap::Parser;
use miette::{IntoDiagnostic, Result};
use tokio::time::sleep;

use crate::actions::{
	iti::{lcd::{
		json::{Item, Screen},
		send,
	}, parse_friendly_duration},
	Context,
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
	#[arg(long, value_parser = parse_friendly_duration)]
	pub watch: Option<Duration>,
}

pub async fn run(args: TemperatureArgs, _ctx: Context) -> Result<()> {
	if let Some(n) = args.watch {
		loop {
			once(&args).await?;
			sleep(n).await;
		}
	} else {
		once(&args).await
	}
}

pub async fn once(args: &TemperatureArgs) -> Result<()> {
	let temperature = duct::cmd!("vcgencmd", "measure_temp")
		.read()
		.into_diagnostic()?
		.trim_start_matches("temp=")
		.trim_end_matches("'C")
		.parse::<f32>()
		.into_diagnostic()?;

	if args.json {
		println!(
			"{}",
			serde_json::json!({
				"temperature": temperature,
			})
		);
	} else {
		println!("{:.1}°C", temperature);
	}

	#[cfg(feature = "iti-lcd")]
	if let Some(y) = args.update_screen {
		const GREEN: [u8; 3] = [0, 255, 0];
		const RED: [u8; 3] = [255, 0, 0];
		const BLACK: [u8; 3] = [0, 0, 0];
		const YELLOW: [u8; 3] = [255, 255, 0];

		send(
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
					stroke: Some(if temperature < 60.0 {
						GREEN
					} else if temperature > 80.0 {
						RED
					} else {
						YELLOW
					}),
					text: Some(format!("{temperature:>5.1}C")),
					..Default::default()
				},
			]),
		)?;
	}

	Ok(())
}
