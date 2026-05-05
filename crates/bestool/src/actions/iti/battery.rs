use std::time::Duration;

use clap::Parser;
use folktime::duration::{Duration as Folktime, Style as Folkstyle};
use miette::Result;
use tokio::time::sleep;
use tracing::instrument;

use crate::actions::{
	Context,
	iti::{
		ItiArgs,
		samplers::battery::{BatteryEstimate, BatteryEstimator, BatterySample, sample},
	},
};
#[cfg(feature = "iti-lcd")]
use crate::actions::iti::lcd::{
	json::{Item, Screen},
	send,
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

pub async fn run(ctx: Context<ItiArgs, BatteryArgs>) -> Result<()> {
	if let Some(n) = ctx.args_sub.watch {
		let interval: Duration = *n;
		let mut estimator = ctx
			.args_sub
			.estimate
			.then(|| BatteryEstimator::new(interval));

		// First round under --estimate is sample-only, to seed the rolling window.
		if let Some(est) = estimator.as_mut() {
			let s = sample()?;
			est.observe(s.capacity);
			report(&ctx.args_sub, s, None);
			sleep(interval).await;
		}

		loop {
			let s = sample()?;
			let estimate = estimator.as_mut().map(|e| e.observe(s.capacity));
			report(&ctx.args_sub, s, estimate);
			sleep(interval).await;
		}
	} else {
		let s = sample()?;
		report(&ctx.args_sub, s, None);
		Ok(())
	}
}

#[instrument(level = "debug", skip(args, estimate))]
fn report(args: &BatteryArgs, sample: BatterySample, estimate: Option<BatteryEstimate>) {
	let BatterySample {
		vcell,
		capacity,
		version,
		powered,
	} = sample;
	let display_capacity = estimate.map(|e| e.capacity).unwrap_or(capacity);

	let status: &str = if let Some(est) = estimate.as_ref() {
		est.status
	} else if powered {
		"charging"
	} else {
		// "powered" is frequently false-negative so we can't rely on it for discharging.
		"unknown"
	};

	if args.json {
		if let Some(est) = estimate.as_ref() {
			let time_remaining_pretty = est.time_remaining.map(folktime_pretty);
			println!(
				"{}",
				serde_json::json!({
					"status": status,
					"vcell": vcell,
					"capacity": display_capacity,
					"version": version,
					"rate": est.rate_per_second,
					"time_remaining": est.time_remaining.map(|d| d.as_secs()),
					"time_remaining_pretty": time_remaining_pretty.as_ref().map(ToString::to_string),
				})
			);
		} else {
			println!(
				"{}",
				serde_json::json!({
					"status": status,
					"vcell": vcell,
					"capacity": display_capacity,
					"version": version,
				})
			);
		}
	} else {
		println!("Version: {version}");
		println!("Voltage: {vcell:.2}V");
		println!("Battery: {display_capacity:.2}%");
		if let Some(est) = estimate.as_ref() {
			println!(
				"Rate: {:.2}%/h ({status})",
				est.rate_per_second * 60.0 * 60.0,
			);
			if let Some(time_remaining) = est.time_remaining {
				println!("Time remaining: {}", folktime_pretty(time_remaining));
			}
		}
	}

	#[cfg(feature = "iti-lcd")]
	if let Some(y) = args.update_screen {
		update_screen(args, y, display_capacity, estimate);
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

#[cfg(feature = "iti-lcd")]
fn update_screen(
	args: &BatteryArgs,
	y: i32,
	capacity: f64,
	estimate: Option<BatteryEstimate>,
) {
	const GREEN: [u8; 3] = [0, 255, 0];
	const RED: [u8; 3] = [255, 0, 0];
	const BLACK: [u8; 3] = [0, 0, 0];
	const WHITE: [u8; 3] = [255, 255, 255];

	let charging = estimate.is_some_and(|e| e.rate_per_second > 0.0);
	let (fill, stroke) = if charging {
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

	let (bg_x, bg_w) = if let Some((rate, time_remaining)) = estimate
		.and_then(|e| e.time_remaining.map(|d| (e.rate_per_second, d)))
	{
		items.push(Item {
			x: 20,
			y,
			stroke: Some(stroke),
			text: Some(if rate < 0.0 {
				format!("{} left", folktime_pretty(time_remaining))
			} else {
				format!("full in {}", folktime_pretty(time_remaining))
			}),
			..Default::default()
		});
		(18, 254)
	} else if estimate.is_some_and(|e| e.rate_per_second == 0.0) && capacity == 100.0 {
		items.push(Item {
			x: 20,
			y,
			stroke: Some(stroke),
			text: Some("fully charged".into()),
			..Default::default()
		});
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

	let _ = send(&args.zmq_socket, Screen::Layout(items));
}
