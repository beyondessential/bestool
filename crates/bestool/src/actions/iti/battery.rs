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

/// Get battery information from the X1201 board.
#[derive(Debug, Clone, Parser)]
pub struct BatteryArgs {
	/// Output in JSON format.
	#[arg(long)]
	pub json: bool,

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
