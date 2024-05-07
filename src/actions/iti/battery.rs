use std::{collections::VecDeque, time::Duration};

use clap::Parser;
use miette::{IntoDiagnostic, Result, WrapErr};
use rppal::{gpio::Gpio, i2c::I2c};
use tokio::time::sleep;
use tracing::instrument;

use crate::actions::{
	iti::lcd::{
		json::{Item, Screen},
		send,
	},
	Context,
};

/// Get battery information from the X1201 board.
#[derive(Debug, Clone, Parser)]
pub struct BatteryArgs {
	/// Output in JSON format.
	#[arg(long)]
	pub json: bool,

	/// Update screen with battery status.
	///
	/// Argument is the Y position of the battery status. The X position is always 240 (right edge).
	///
	/// With --estimate, this will also print the time remaining on the left edge (X=20).
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

	/// With --watch, also estimate charging rate and time remaining.
	///
	/// The first round will be estimate-less, as it is used to gather data. After that, the rate
	/// and time remaining (in seconds in the JSON output) are calculated on a rolling basis.
	#[arg(long)]
	pub estimate: bool,
}

pub async fn run(ctx: Context<BatteryArgs>) -> Result<()> {
	if let Some(n) = ctx.args_top.watch {
		let n = n.as_ref().clone();

		// gather info only for initial round
		let mut rolling = if ctx.args_top.estimate {
			let first = once(ctx.clone(), None).await?;
			sleep(n).await;
			Some(VecDeque::from([first]))
		} else {
			None
		};

		loop {
			once(ctx.clone(), rolling.as_mut()).await?;
			sleep(n).await;
		}
	} else {
		once(ctx, None).await?;
	}

	Ok(())
}

pub async fn once(ctx: Context<BatteryArgs>, rolling: Option<&mut VecDeque<f64>>) -> Result<f64> {
	let gpio = Gpio::new().into_diagnostic().wrap_err("gpio: init")?;
	let powered = gpio
		.get(6)
		.into_diagnostic()
		.wrap_err("gpio: read pin=6")?
		.into_input()
		.is_high();

	let mut i2c = I2c::new().into_diagnostic().wrap_err("i2c: init")?;
	i2c.set_slave_address(0x36)
		.into_diagnostic()
		.wrap_err("i2c: set address")?;

	// https://www.analog.com/media/en/technical-documentation/data-sheets/MAX17048-MAX17049.pdf
	let vcell = (read(&mut i2c, 0x2)? as f64) * 1.25 / 1000.0 / 16.0;
	let capacity = ((read(&mut i2c, 0x4)? as f64) / 256.0).clamp(0.0, 100.0);
	let version = read(&mut i2c, 0x8)?;

	let estimates = if let Some(rolling) = rolling {
		rolling.push_back(capacity);
		let front = if rolling.len() > 100 {
			rolling.pop_front()
		} else {
			rolling.front().copied()
		}
		.expect("rolling is always non-empty");

		let rate = (capacity - front)
			/ ((rolling.len() as u64 * ctx.args_top.watch.unwrap().as_ref().as_secs()) as f64);
		let capacity_left = if rate > 0.0 {
			(100.0 - capacity).abs()
		} else {
			capacity
		}
		.clamp(0.0, 100.0);
		let time_remaining = capacity_left / rate.abs();
		Some((
			rate,
			if time_remaining.is_finite() {
				Some(humantime::Duration::from(Duration::from_secs(
					time_remaining as _,
				)))
			} else {
				None
			},
		))
	} else {
		None
	};

	if ctx.args_top.json {
		if let Some((rate, time_remaining)) = estimates {
			println!(
				"{}",
				serde_json::json!({
					"powered": powered,
					"vcell": vcell,
					"capacity": capacity,
					"version": version,
					"rate": rate,
					"status": if rate > 0.0 { "charging" } else if rate < 0.0 { "discharging" } else { "stable" },
					"time_remaining": time_remaining.map(|d| d.as_secs()),
					"time_remaining_pretty": time_remaining.map(|d| d.to_string()),
				})
			);
		} else {
			println!(
				"{}",
				serde_json::json!({ "powered": powered, "vcell": vcell, "capacity": capacity, "version": version })
			);
		}
	} else {
		println!("Powered: {}", powered);
		println!("Version: {}", version);
		println!("Voltage: {:.2} V", vcell);
		println!("Battery: {:.2}%", capacity);
		if let Some((rate, time_remaining)) = estimates {
			println!(
				"Rate: {:.2}%/h ({})",
				rate * 60.0 * 60.0,
				if rate > 0.0 {
					"charging"
				} else if rate < 0.0 {
					"discharging"
				} else {
					"stable"
				}
			);
			if let Some(time_remaining) = time_remaining {
				println!("Time remaining: {time_remaining}");
			}
		}
	}

	#[cfg(feature = "iti-lcd")]
	if let Some(y) = ctx.args_top.update_screen {
		let fill = if estimates.map_or(false, |(rate, _)| rate > 0.0) {
			[0, 255, 0]
		} else if capacity < 3.0 {
			[255, 0, 0]
		} else {
			[255, 255, 255]
		};
		let stroke = if capacity < 3.0 {
			[255, 255, 255]
		} else if capacity <= 15.0 {
			[200, 0, 0]
		} else {
			[0, 0, 0]
		};

		let mut items = vec![Item {
			x: 240,
			y,
			stroke: Some(stroke),
			text: Some(format!("{capacity:>2.0}%")),
			..Default::default()
		}];

		let (bg_x, bg_w) = if let Some(time_remaining) = estimates.and_then(|(_, time_remaining)| time_remaining) {
			items.push(Item {
				x: 20,
				y,
				stroke: Some(stroke),
				text: Some(format!("{time_remaining} left")),
				..Default::default()
			});
			(18, 254)
		} else if estimates.map_or(false, |(rate, _)| rate == 0.0) {
			// when stable, also erase the time remaining
			(18, 254)
		} else {
			(238, 34)
		};

		items.insert(
			0,
			Item {
				x: bg_x,
				y: y - 16,
				width: Some(bg_w),
				height: Some(20),
				fill: Some(fill),
				..Default::default()
			},
		);

		send(&ctx.args_top.zmq_socket, Screen::Layout(items))?;
	}

	Ok(capacity)
}

#[instrument(level = "debug", skip(i2c))]
fn read(i2c: &mut I2c, addr: u8) -> Result<u16> {
	let data = i2c
		.smbus_read_word(addr)
		.into_diagnostic()
		.wrap_err(format!("i2c: read {addr:2X?}"))?;
	Ok(u16::from_le_bytes(data.to_be_bytes()))
}
