//! Prometheus metrics for the alertd daemon.
//!
//! Tracks the following metrics:
//! - `bes_alertd_alerts_loaded`: Number of alerts currently loaded (gauge)
//! - `bes_alertd_alerts_sent_total`: Total number of alerts sent successfully (counter)
//! - `bes_alertd_alerts_failed_total`: Total number of alerts that failed to send (counter)
//! - `bes_alertd_reloads_total`: Total number of configuration reloads (counter)

use std::sync::OnceLock;

use miette::{IntoDiagnostic, Result};
use prometheus::{Encoder, IntCounter, IntGauge, Registry, TextEncoder};

static REGISTRY: OnceLock<Registry> = OnceLock::new();
static ALERTS_LOADED: OnceLock<IntGauge> = OnceLock::new();
static ALERTS_SENT_TOTAL: OnceLock<IntCounter> = OnceLock::new();
static ALERTS_FAILED_TOTAL: OnceLock<IntCounter> = OnceLock::new();
static RELOADS_TOTAL: OnceLock<IntCounter> = OnceLock::new();

pub fn init_metrics() {
	let registry = Registry::new();

	let alerts_loaded = IntGauge::new(
		"bes_alertd_alerts_loaded",
		"Number of alerts currently loaded",
	)
	.expect("failed to create alerts_loaded metric");

	let alerts_sent_total = IntCounter::new(
		"bes_alertd_alerts_sent_total",
		"Total number of alerts sent",
	)
	.expect("failed to create alerts_sent_total metric");

	let alerts_failed_total = IntCounter::new(
		"bes_alertd_alerts_failed_total",
		"Total number of alerts that failed to send",
	)
	.expect("failed to create alerts_failed_total metric");

	let reloads_total = IntCounter::new(
		"bes_alertd_reloads_total",
		"Total number of configuration reloads",
	)
	.expect("failed to create reloads_total metric");

	registry
		.register(Box::new(alerts_loaded.clone()))
		.expect("failed to register alerts_loaded metric");
	registry
		.register(Box::new(alerts_sent_total.clone()))
		.expect("failed to register alerts_sent_total metric");
	registry
		.register(Box::new(alerts_failed_total.clone()))
		.expect("failed to register alerts_failed_total metric");
	registry
		.register(Box::new(reloads_total.clone()))
		.expect("failed to register reloads_total metric");

	REGISTRY.set(registry).expect("metrics already initialized");
	ALERTS_LOADED
		.set(alerts_loaded)
		.expect("metrics already initialized");
	ALERTS_SENT_TOTAL
		.set(alerts_sent_total)
		.expect("metrics already initialized");
	ALERTS_FAILED_TOTAL
		.set(alerts_failed_total)
		.expect("metrics already initialized");
	RELOADS_TOTAL
		.set(reloads_total)
		.expect("metrics already initialized");
}

pub fn set_alerts_loaded(count: usize) {
	if let Some(metric) = ALERTS_LOADED.get() {
		metric.set(count as i64);
	}
}

pub fn inc_alerts_sent() {
	if let Some(metric) = ALERTS_SENT_TOTAL.get() {
		metric.inc();
	}
}

pub fn inc_alerts_failed() {
	if let Some(metric) = ALERTS_FAILED_TOTAL.get() {
		metric.inc();
	}
}

pub fn inc_reloads() {
	if let Some(metric) = RELOADS_TOTAL.get() {
		metric.inc();
	}
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
