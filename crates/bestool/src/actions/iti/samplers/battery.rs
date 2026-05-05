use std::{collections::VecDeque, time::Duration};

use miette::{IntoDiagnostic, Result, WrapErr};
use rppal::{gpio::Gpio, i2c::I2c};
use tracing::instrument;

/// One reading from the X1201 UPS board.
#[derive(Debug, Clone, Copy)]
pub struct BatterySample {
	/// Per-cell voltage in volts.
	pub vcell: f64,
	/// Charge percentage in `0.0..=100.0`.
	pub capacity: f64,
	/// MAX17048 version register (returned for diagnostic logging).
	pub version: u16,
	/// True if the GPIO power-detect pin is high.
	pub powered: bool,
}

/// Read one sample from the MAX17048 fuel-gauge over I2C and the powered GPIO.
///
/// References the MAX17048 datasheet §"Register Summary":
/// <https://www.analog.com/media/en/technical-documentation/data-sheets/MAX17048-MAX17049.pdf>.
#[instrument(level = "debug")]
pub fn sample() -> Result<BatterySample> {
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

	let vcell = (read_register(&mut i2c, 0x2)? as f64) * 1.25 / 1000.0 / 16.0;
	let capacity = ((read_register(&mut i2c, 0x4)? as f64) / 256.0).clamp(0.0, 100.0);
	let version = read_register(&mut i2c, 0x8)?;

	Ok(BatterySample {
		vcell,
		capacity,
		version,
		powered,
	})
}

#[instrument(level = "trace", skip(i2c))]
fn read_register(i2c: &mut I2c, addr: u8) -> Result<u16> {
	let data = i2c
		.smbus_read_word(addr)
		.into_diagnostic()
		.wrap_err(format!("i2c: read {addr:2X?}"))?;
	Ok(u16::from_le_bytes(data.to_be_bytes()))
}

/// Rolling-window estimator for charge rate and time-remaining.
///
/// Records up to 100 capacity samples taken at a fixed interval. The first sample is the most
/// recent. The estimator does several heuristic adjustments to avoid producing nonsense
/// time-remaining values from noisy short-window data; see [`BatteryEstimator::observe`].
pub struct BatteryEstimator {
	interval: Duration,
	window: VecDeque<f64>,
}

impl BatteryEstimator {
	pub fn new(interval: Duration) -> Self {
		Self {
			interval,
			window: VecDeque::new(),
		}
	}

	/// Push a new capacity sample and produce a best-effort estimate.
	pub fn observe(&mut self, capacity: f64) -> BatteryEstimate {
		self.window.push_front(capacity);
		self.window.truncate(100);

		// Walk the window from "now" backwards, looking for the first interval at which the
		// capacity differs from the most recent sample, with a minimum lookback of 5 intervals
		// (or shorter if we don't have that much history yet).
		let index_to_first_difference = self
			.window
			.iter()
			.scan(self.window.front().copied().unwrap_or(capacity), |prev, curr| {
				let pre = *prev;
				*prev = *curr;
				Some(curr - pre)
			})
			.enumerate()
			.find(|(n, diff)| *n >= 4.min(self.window.len() - 1) && *diff != 0.0)
			.map(|(n, _)| n)
			.unwrap_or(self.window.len() - 1);

		let mut rate = (capacity
			- self
				.window
				.get(index_to_first_difference)
				.copied()
				.unwrap_or(capacity))
			/ ((self.window.len() as u64 * self.interval.as_secs()) as f64);
		let mut adjusted_capacity = capacity;
		let capacity_left = if rate > 0.0 {
			(100.0 - capacity).abs()
		} else {
			capacity
		}
		.clamp(0.0, 100.0);

		// Heuristic adjustments (kept verbatim from the original `iti battery` implementation
		// — see the inline comments there for the why).
		if capacity >= 98.5 && rate >= 0.0 {
			adjusted_capacity = 100.0;
			rate = 0.0;
		} else if rate.abs() < 0.00025 {
			rate = 0.0;
		} else if rate.abs() < 0.005 {
			rate = rate.signum() * 0.005;
		}

		let time_remaining = compute_time_remaining(capacity_left, rate);
		let status = if rate > 0.0 {
			"charging"
		} else if rate < 0.0 {
			"discharging"
		} else {
			"stable"
		};

		BatteryEstimate {
			capacity: adjusted_capacity,
			rate_per_second: rate,
			time_remaining,
			status,
		}
	}
}

fn compute_time_remaining(capacity_left: f64, rate_per_second: f64) -> Option<Duration> {
	if rate_per_second == 0.0 {
		return None;
	}
	let secs = capacity_left / rate_per_second.abs();
	if !secs.is_finite() {
		return None;
	}
	let mut dur = Duration::from_secs(secs as u64);
	// Clamp at 6h: the device doesn't last that long, and doesn't take that long to charge.
	if dur > Duration::from_secs(6 * 60 * 60) {
		dur = Duration::from_secs(6 * 60 * 60);
	}
	if dur < Duration::from_secs(5 * 60) {
		None
	} else {
		Some(dur)
	}
}

/// Estimated charging trend, derived from a window of [`BatterySample`]s.
#[derive(Debug, Clone, Copy)]
pub struct BatteryEstimate {
	/// Capacity, possibly clamped to 100.0 when very near full and not discharging.
	pub capacity: f64,
	/// Charge rate in percentage points per second (positive = charging).
	pub rate_per_second: f64,
	/// Estimated time to fully charge / fully discharge, if rate is non-zero and the result is
	/// between 5 minutes and 6 hours.
	pub time_remaining: Option<Duration>,
	/// `"charging"`, `"discharging"`, or `"stable"`.
	pub status: &'static str,
}
