//! Typed metrics a healthcheck can declare for the alertd `/metrics` endpoint.
//!
//! A check computes its numbers once and may attach them both to its canopy
//! payload (via `details`) and, as typed [`Stat`]s, to the metrics surface. The
//! `/metrics` endpoint renders the declared stats to munin or prometheus text;
//! the check name provides the metric namespace and `labels` carry dimensioned
//! data (a status, an HTTP code, a mountpoint).

use jiff::Timestamp;

/// The kind of a declared metric, matching the subset of metric types munin and
/// prometheus have in common.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatKind {
	/// A value that can rise and fall (prometheus gauge, munin `GAUGE`).
	Gauge,
	/// A cumulative total that only increases until whatever produces it
	/// restarts (prometheus counter, munin `DERIVE` floored at zero).
	Counter,
}

impl StatKind {
	/// The prometheus `# TYPE` token.
	pub fn prometheus(self) -> &'static str {
		match self {
			StatKind::Gauge => "gauge",
			StatKind::Counter => "counter",
		}
	}

	/// The munin field `.type` token.
	///
	/// A counter renders as `DERIVE`, not `COUNTER`: rrdtool's `COUNTER` reads a
	/// value below its predecessor as hardware counter wrap and corrects for it
	/// against 32- then 64-bit bounds, turning the reset that follows a Caddy or
	/// worker restart into an astronomical rate spike that wrecks the graph's
	/// autoscale for as long as the RRA keeps it. `DERIVE` takes the plain
	/// difference, so paired with a zero floor (see [`Self::munin_min`]) a reset
	/// reads as the gap it is.
	pub fn munin(self) -> &'static str {
		match self {
			StatKind::Gauge => "GAUGE",
			StatKind::Counter => "DERIVE",
		}
	}

	/// The munin field `.min`, where the kind needs one.
	pub fn munin_min(self) -> Option<&'static str> {
		match self {
			StatKind::Gauge => None,
			StatKind::Counter => Some("0"),
		}
	}
}

/// One numeric metric declared by a healthcheck.
///
/// Build with [`Stat::gauge`] / [`Stat::counter`] and attach to a check with
/// `Check::with_stat`. Units belong in the `name` by prometheus convention
/// (`_seconds`, `_bytes`); dimensioned data is expressed with [`Stat::label`],
/// called once per dimension value.
#[derive(Debug, Clone)]
pub struct Stat {
	/// Metric name within the check's namespace; snake_case, valid as both a
	/// prometheus name segment and a munin field.
	pub name: &'static str,
	pub value: f64,
	pub kind: StatKind,
	/// Dimension labels in insertion order: static keys, dynamic values.
	pub labels: Vec<(&'static str, String)>,
	/// Human description: prometheus `# HELP`, munin field label.
	pub help: Option<String>,
	/// Munin graph group within the check: metrics sharing a group render as
	/// fields of one graph. `None` gives the metric its own graph, named for
	/// it. Only meaningful for munin; prometheus dimensions everything by name
	/// and labels regardless.
	pub group: Option<&'static str>,
	/// Metric namespace overriding the check name in the prometheus metric name
	/// and munin graph. `None` uses the check name. Set when the check name
	/// would mislead — e.g. the error-rate check `http_errors` publishing its
	/// request telemetry (which counts 2xx responses too) under `http`.
	pub namespace: Option<&'static str>,
}

impl Stat {
	pub fn gauge(name: &'static str, value: f64) -> Self {
		Self {
			name,
			value,
			kind: StatKind::Gauge,
			labels: Vec::new(),
			help: None,
			group: None,
			namespace: None,
		}
	}

	pub fn counter(name: &'static str, value: f64) -> Self {
		Self {
			name,
			value,
			kind: StatKind::Counter,
			labels: Vec::new(),
			help: None,
			group: None,
			namespace: None,
		}
	}

	/// Attach a dimension label. Call once per value for dimensioned data (e.g.
	/// once per FHIR job status).
	pub fn label(mut self, key: &'static str, value: impl Into<String>) -> Self {
		self.labels.push((key, value.into()));
		self
	}

	pub fn help(mut self, help: impl Into<String>) -> Self {
		self.help = Some(help.into());
		self
	}

	/// Place this metric in a named munin graph group, so metrics that share a
	/// unit and are read together render on one graph. Metrics of different
	/// units should stay in separate groups (the default: their own graph).
	pub fn group(mut self, group: &'static str) -> Self {
		self.group = Some(group);
		self
	}

	/// Publish this metric under `namespace` instead of the check's name. See
	/// [`Self::namespace`].
	pub fn namespace(mut self, namespace: &'static str) -> Self {
		self.namespace = Some(namespace);
		self
	}

	/// The metric namespace: the override if set, else the check name.
	pub fn namespace_or<'a>(&self, check: &'a str) -> &'a str {
		match self.namespace {
			Some(ns) => ns,
			None => check,
		}
	}
}

/// Census of check outcomes in a sweep, capped to canopy's severity ceilings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusCounts {
	pub passing: u32,
	pub warning: u32,
	pub failing: u32,
	pub skipped: u32,
	pub broken: u32,
}

impl StatusCounts {
	/// Every check in the sweep.
	pub fn total(&self) -> u32 {
		self.passing + self.warning + self.failing + self.skipped + self.broken
	}

	/// Checks that actually ran (everything but skipped).
	pub fn active(&self) -> u32 {
		self.total() - self.skipped
	}

	/// The per-state counts in the stable order both metric formats emit.
	pub fn by_state(&self) -> [(&'static str, u32); 5] {
		[
			("passing", self.passing),
			("warning", self.warning),
			("failing", self.failing),
			("skipped", self.skipped),
			("broken", self.broken),
		]
	}
}

/// A rendering-ready view of the latest sweep's metrics: the per-check declared
/// stats plus the global status census, both drawn from one sweep so they stay
/// internally consistent.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
	pub computed_at: Timestamp,
	/// `(check name, stat)` pairs; the check name is the metric namespace.
	pub stats: Vec<(&'static str, Stat)>,
	pub counts: StatusCounts,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn gauge_builder_defaults() {
		let s = Stat::gauge("age_seconds", 42.0);
		assert_eq!(s.name, "age_seconds");
		assert_eq!(s.value, 42.0);
		assert_eq!(s.kind, StatKind::Gauge);
		assert!(s.labels.is_empty());
		assert!(s.help.is_none());
	}

	#[test]
	fn counter_kind() {
		assert_eq!(Stat::counter("requests_total", 1.0).kind, StatKind::Counter);
	}

	#[test]
	fn labels_keep_insertion_order() {
		let s = Stat::gauge("jobs", 3.0)
			.label("status", "Queued")
			.label("queue", "fhir");
		assert_eq!(
			s.labels,
			vec![
				("status", "Queued".to_string()),
				("queue", "fhir".to_string()),
			]
		);
	}

	#[test]
	fn help_is_attached() {
		let s = Stat::gauge("x", 1.0).help("a thing");
		assert_eq!(s.help.as_deref(), Some("a thing"));
	}

	#[test]
	fn group_defaults_none_and_is_attachable() {
		assert!(Stat::gauge("x", 1.0).group.is_none());
		assert_eq!(
			Stat::gauge("used_bytes", 1.0).group("bytes").group,
			Some("bytes")
		);
	}

	#[test]
	fn namespace_defaults_to_check_and_overrides() {
		assert_eq!(
			Stat::gauge("x", 1.0).namespace_or("http_errors"),
			"http_errors"
		);
		assert_eq!(
			Stat::gauge("x", 1.0)
				.namespace("http")
				.namespace_or("http_errors"),
			"http"
		);
	}

	#[test]
	fn kind_wire_tokens() {
		assert_eq!(StatKind::Gauge.prometheus(), "gauge");
		assert_eq!(StatKind::Counter.prometheus(), "counter");
		assert_eq!(StatKind::Gauge.munin(), "GAUGE");
		// DERIVE, not COUNTER: rrdtool reads a COUNTER's decrease as hardware
		// wrap and fabricates an enormous rate for the interval.
		assert_eq!(StatKind::Counter.munin(), "DERIVE");
	}

	#[test]
	fn a_counter_is_floored_at_zero() {
		// Without the floor a DERIVE would graph the negative jump a reset makes.
		assert_eq!(StatKind::Counter.munin_min(), Some("0"));
		assert_eq!(StatKind::Gauge.munin_min(), None);
	}
}
