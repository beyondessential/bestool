use std::{collections::VecDeque, time::Duration};

use clap::Parser;
use folktime::duration::{Duration as Folktime, Style as Folkstyle};
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
	let mut capacity = ((read(&mut i2c, 0x4)? as f64) / 256.0).clamp(0.0, 100.0);
	let version = read(&mut i2c, 0x8)?;

	let estimates = if let Some(rolling) = rolling {
		rolling.push_front(capacity);
		rolling.truncate(100);
		// [now, interval ago, ..., 99 intervals ago]

		// look back and find the first time the value changed
		// that is at least 5 intervals away, data-permitting.
		let index_to_first_difference = rolling
			.iter()
			.scan(rolling.front().unwrap(), |prev, curr| {
				let pre = *prev;
				*prev = curr;
				Some(curr - pre)
			})
			.enumerate()
			.filter(|(n, diff)| *n >= 4.min(rolling.len() - 1) && *diff != 0.0)
			.next()
			.map(|(n, _)| n)
			.unwrap_or(rolling.len() - 1);

		let mut rate = (capacity - rolling.get(index_to_first_difference).unwrap_or(&capacity))
			/ ((rolling.len() as u64 * ctx.args_top.watch.unwrap().as_ref().as_secs()) as f64);
		let capacity_left = if rate > 0.0 {
			(100.0 - capacity).abs()
		} else {
			capacity
		}
		.clamp(0.0, 100.0);

		if capacity >= 98.5 && rate >= 0.0 {
			// fudge full capacity if it's close enough and we're "charging"
			// otherwise we get non-sensical time remaining like "7 days to reach 100%"
			capacity = 100.0;
			rate = 0.0;
		} else if rate.abs() < 0.00025 {
			// fudge rate if it's close enough to zero
			rate = 0.0;
		} else if rate.abs() < 0.005 {
			// fudge rate to a higher value if it's not zeroish but too low to produce good estimates
			rate = rate.signum() * 0.005;
		}

		// TODO: replace the fudging with a better algorithm (e.g. exponential smoothing)
		//       or better yet, store historical data and calibrate estimates from that.

		let time_remaining = capacity_left / rate.abs();
		let time_remaining = if time_remaining.is_finite() {
			let mut dur = Duration::from_secs(time_remaining as _);
			if dur > Duration::from_secs(6 * 60 * 60) {
				// clamp time remaining in either direction to 6 hours
				// we know that the iti doesn't last that long, and doesn't take that long to charge
				dur = Duration::from_secs(6 * 60 * 60);
			}

			// only show time remaining if it's more than 5 minutes
			if dur < Duration::from_secs(5 * 60) {
				None
			} else {
				Some(dur)
			}
		} else {
			None
		};

		Some((
			rate,
			time_remaining.map(|dur| {
				Folktime(
					dur,
					if dur > Duration::from_secs(60 * 60) {
						Folkstyle::TwoUnitsWhole
					} else {
						Folkstyle::OneUnitWhole
					},
				)
			}),
		))
	} else {
		None
	};

	let status = if let Some((rate, _)) = estimates {
		if rate > 0.0 {
			"charging"
		} else if rate < 0.0 {
			"discharging"
		} else {
			"stable"
		}
	} else {
		if powered {
			"charging"
		} else {
			// "powered" is frequently false-negative so we can't rely on it for discharging
			"unknown"
		}
	};

	if ctx.args_top.json {
		if let Some((rate, ref time_remaining)) = estimates {
			println!(
				"{}",
				serde_json::json!({
					"status": status,
					"vcell": vcell,
					"capacity": capacity,
					"version": version,
					"rate": rate,
					"time_remaining": time_remaining.as_ref().map(|d| d.0.as_secs()),
					"time_remaining_pretty": time_remaining.as_ref().map(|d| d.to_string()),
				})
			);
		} else {
			println!(
				"{}",
				serde_json::json!({ "status": status, "vcell": vcell, "capacity": capacity, "version": version })
			);
		}
	} else {
		println!("Version: {}", version);
		println!("Voltage: {:.2}V", vcell);
		println!("Battery: {:.2}%", capacity);
		if let Some((rate, ref time_remaining)) = estimates {
			println!("Rate: {:.2}%/h ({status})", rate * 60.0 * 60.0,);
			if let Some(time_remaining) = time_remaining {
				println!("Time remaining: {time_remaining}");
			}
		}
	}

	#[cfg(feature = "iti-lcd")]
	if let Some(y) = ctx.args_top.update_screen {
		const GREEN: [u8; 3] = [0, 255, 0];
		const RED: [u8; 3] = [255, 0, 0];
		const BLACK: [u8; 3] = [0, 0, 0];
		const WHITE: [u8; 3] = [255, 255, 255];

		let (fill, stroke) = if estimates.as_ref().map_or(false, |(rate, _)| *rate > 0.0) {
			(GREEN, BLACK)
		} else if capacity <= 3.0 {
			(RED, WHITE)
		} else if capacity <= 15.0 {
			(BLACK, RED)
		} else {
			(BLACK, WHITE)
		};

		let mut items = vec![Item {
			x: 230,
			y,
			stroke: Some(stroke),
			text: Some(format!("{capacity:>3.0}%")),
			..Default::default()
		}];

		let (bg_x, bg_w) = if let Some((rate, time_remaining)) = estimates
			.as_ref()
			.and_then(|(rate, time_remaining)| time_remaining.as_ref().map(|d| (rate, d)))
		{
			items.push(Item {
				x: 20,
				y,
				stroke: Some(stroke),
				text: Some(if *rate < -0.0 {
					format!("{time_remaining} left")
				} else {
					format!("full in {time_remaining}")
				}),
				..Default::default()
			});
			(18, 254)
		} else if estimates.map_or(false, |(rate, _)| !(rate > 0.0) && !(rate < -0.0)) {
			if capacity == 100.0 {
				items.push(Item {
					x: 20,
					y,
					stroke: Some(stroke),
					text: Some("fully charged".into()),
					..Default::default()
				});
			}
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
