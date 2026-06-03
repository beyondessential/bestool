//! Prometheus metrics for the alertd daemon.
//!
//! Tracks the following metrics:
//! - `bes_alertd_last_activity_unix`: Unix timestamp of the last activity (gauge)

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, Ordering};

use jiff::Timestamp;
use miette::{IntoDiagnostic, Result};
use prometheus::{Encoder, IntGauge, Registry, TextEncoder};

static REGISTRY: OnceLock<Registry> = OnceLock::new();
static LAST_ACTIVITY_GAUGE: OnceLock<IntGauge> = OnceLock::new();
static LAST_ACTIVITY: AtomicI64 = AtomicI64::new(0);

pub fn init_metrics() {
	let registry = Registry::new();

	let last_activity_gauge = IntGauge::new(
		"bes_alertd_last_activity_unix",
		"Unix timestamp of the last activity",
	)
	.expect("failed to create last_activity_gauge metric");

	registry
		.register(Box::new(last_activity_gauge.clone()))
		.expect("failed to register last_activity_gauge metric");

	REGISTRY.set(registry).expect("metrics already initialized");
	LAST_ACTIVITY_GAUGE
		.set(last_activity_gauge)
		.expect("metrics already initialized");
}

pub fn record_activity() {
	let now = Timestamp::now().as_second();
	LAST_ACTIVITY.store(now, Ordering::Relaxed);
	if let Some(metric) = LAST_ACTIVITY_GAUGE.get() {
		metric.set(now);
	}
}

pub fn last_activity_timestamp() -> i64 {
	LAST_ACTIVITY.load(Ordering::Relaxed)
}

pub fn gather_metrics() -> Result<String> {
	let registry = REGISTRY
		.get()
		.ok_or_else(|| miette::miette!("metrics not initialized"))?;
	let metric_families = registry.gather();
	let encoder = TextEncoder::new();
	let mut buffer = Vec::new();
	encoder
		.encode(&metric_families, &mut buffer)
		.into_diagnostic()?;
	String::from_utf8(buffer).into_diagnostic()
}
