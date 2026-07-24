//! Render a [`MetricsSnapshot`] to prometheus or munin text.
//!
//! One snapshot drives both formats. Prometheus models dimensioned data with
//! labels (`bes_alertd_fhir_jobs_jobs{status="Queued"}`); munin models it as
//! fields within a per-check multigraph. The liveness/sweep timestamps ride
//! along in both so a scraper can tell whether the daemon (and its last sweep)
//! is fresh.

use std::fmt::Write as _;

use crate::doctor::{MetricsSnapshot, Stat};

/// Common prefix for every metric this daemon exposes.
const PREFIX: &str = "bes_alertd";

/// The five census states, in a stable order for both formats. Values come
/// from [`StatusCounts::by_state`], which uses the same order.
const STATE_NAMES: [&str; 5] = ["passing", "warning", "failing", "skipped", "broken"];

/// Format an `f64` for a metric value. Rust's `Display` prints whole numbers
/// without a trailing `.0` and never uses scientific notation, which is exactly
/// what both formats want.
fn value(v: f64) -> String {
	format!("{v}")
}

/// Prometheus label-value escaping: backslash, double-quote, newline.
fn escape_label(v: &str) -> String {
	v.replace('\\', "\\\\")
		.replace('"', "\\\"")
		.replace('\n', "\\n")
}

/// Sanitise a munin field-name segment to `[a-z0-9_]`.
fn munin_field_segment(s: &str) -> String {
	s.chars()
		.map(|c| {
			if c.is_ascii_alphanumeric() {
				c.to_ascii_lowercase()
			} else {
				'_'
			}
		})
		.collect()
}

/// The munin field id for a stat: the stat name, plus each label value folded
/// in. Always starts with the stat name (a letter), so it's a valid field id.
fn munin_field(stat: &Stat) -> String {
	let mut id = munin_field_segment(stat.name);
	for (_, v) in &stat.labels {
		id.push('_');
		id.push_str(&munin_field_segment(v));
	}
	id
}

/// The human label for a munin field: the label values joined, else the help,
/// else the stat name.
fn munin_field_label(stat: &Stat) -> String {
	if !stat.labels.is_empty() {
		stat.labels
			.iter()
			.map(|(_, v)| v.as_str())
			.collect::<Vec<_>>()
			.join(" ")
	} else if let Some(help) = &stat.help {
		help.clone()
	} else {
		stat.name.to_string()
	}
}

/// The prometheus metric name for a stat within a check's namespace.
fn prom_name(check: &str, stat: &Stat) -> String {
	format!("{PREFIX}_{check}_{}", stat.name)
}

/// Render the snapshot's prometheus body (census + per-check stats). Does not
/// include the liveness gauge, which the caller appends from the registry to
/// preserve its exact existing output.
pub fn render_prometheus(snapshot: &MetricsSnapshot) -> String {
	let mut out = String::new();

	out.push_str(&format!(
		"# HELP {PREFIX}_last_sweep_unix Unix time of the last doctor sweep\n"
	));
	out.push_str(&format!("# TYPE {PREFIX}_last_sweep_unix gauge\n"));
	out.push_str(&format!(
		"{PREFIX}_last_sweep_unix {}\n",
		snapshot.computed_at.as_second()
	));

	out.push_str(&format!(
		"# HELP {PREFIX}_checks Number of doctor checks by outcome\n"
	));
	out.push_str(&format!("# TYPE {PREFIX}_checks gauge\n"));
	for (state, count) in snapshot.counts.by_state() {
		out.push_str(&format!("{PREFIX}_checks{{state=\"{state}\"}} {count}\n"));
	}

	// Group per-check stats into prometheus metric families (same name = same
	// family), preserving first-seen order so output is stable.
	let mut order: Vec<String> = Vec::new();
	let mut families: std::collections::HashMap<String, Family> = std::collections::HashMap::new();
	for (check, stat) in &snapshot.stats {
		let name = prom_name(check, stat);
		let family = families.entry(name.clone()).or_insert_with(|| {
			order.push(name.clone());
			Family {
				help: stat.help.clone(),
				kind: stat.kind.prometheus(),
				lines: Vec::new(),
			}
		});
		if family.help.is_none() {
			family.help.clone_from(&stat.help);
		}
		let labels = if stat.labels.is_empty() {
			String::new()
		} else {
			let inner = stat
				.labels
				.iter()
				.map(|(k, v)| format!("{k}=\"{}\"", escape_label(v)))
				.collect::<Vec<_>>()
				.join(",");
			format!("{{{inner}}}")
		};
		family
			.lines
			.push(format!("{name}{labels} {}", value(stat.value)));
	}

	for name in order {
		let family = &families[&name];
		if let Some(help) = &family.help {
			let _ = writeln!(out, "# HELP {name} {}", help.replace('\n', " "));
		}
		let _ = writeln!(out, "# TYPE {name} {}", family.kind);
		for line in &family.lines {
			out.push_str(line);
			out.push('\n');
		}
	}

	out
}

struct Family {
	help: Option<String>,
	kind: &'static str,
	lines: Vec<String>,
}

/// Render munin text. In `config` mode, field metadata; otherwise, values. The
/// daemon liveness/sweep graph is always emitted; the census and per-check
/// graphs need a sweep (their field sets are only known from one).
pub fn render_munin(
	snapshot: Option<&MetricsSnapshot>,
	last_activity: i64,
	config: bool,
) -> String {
	let mut out = String::new();

	// Daemon graph: liveness and (when available) last-sweep timestamps.
	out.push_str("multigraph bes_alertd_daemon\n");
	if config {
		out.push_str("graph_title alertd daemon activity\n");
		out.push_str("graph_category bestool\n");
		out.push_str("last_activity.label last activity (unix)\n");
		out.push_str("last_activity.type GAUGE\n");
		if snapshot.is_some() {
			out.push_str("last_sweep.label last sweep (unix)\n");
			out.push_str("last_sweep.type GAUGE\n");
		}
	} else {
		let _ = writeln!(out, "last_activity.value {last_activity}");
		if let Some(s) = snapshot {
			let _ = writeln!(out, "last_sweep.value {}", s.computed_at.as_second());
		}
	}

	let Some(snapshot) = snapshot else {
		return out;
	};

	// Census graph.
	out.push_str("\nmultigraph bes_alertd_checks\n");
	if config {
		out.push_str("graph_title Doctor checks by outcome\n");
		out.push_str("graph_category bestool\n");
		out.push_str("graph_vlabel checks\n");
		for state in STATE_NAMES {
			let _ = writeln!(out, "{state}.label {state}");
			let _ = writeln!(out, "{state}.type GAUGE");
		}
		out.push_str("total.label total\n");
		out.push_str("total.type GAUGE\n");
	} else {
		for (state, count) in snapshot.counts.by_state() {
			let _ = writeln!(out, "{state}.value {count}");
		}
		let _ = writeln!(out, "total.value {}", snapshot.counts.total());
	}

	// Per-check graphs, one multigraph per check, in first-seen order.
	let mut order: Vec<&str> = Vec::new();
	let mut by_check: std::collections::HashMap<&str, Vec<&Stat>> =
		std::collections::HashMap::new();
	for (check, stat) in &snapshot.stats {
		by_check
			.entry(check)
			.or_insert_with(|| {
				order.push(check);
				Vec::new()
			})
			.push(stat);
	}

	for check in order {
		let _ = write!(out, "\nmultigraph {PREFIX}_{check}\n");
		let stats = &by_check[check];
		if config {
			let _ = writeln!(out, "graph_title {check}");
			out.push_str("graph_category bestool\n");
			for stat in stats {
				let field = munin_field(stat);
				let _ = writeln!(out, "{field}.label {}", munin_field_label(stat));
				let _ = writeln!(out, "{field}.type {}", stat.kind.munin());
				if let Some(help) = &stat.help {
					let _ = writeln!(out, "{field}.info {}", help.replace('\n', " "));
				}
			}
		} else {
			for stat in stats {
				let _ = writeln!(out, "{}.value {}", munin_field(stat), value(stat.value));
			}
		}
	}

	out
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::StatusCounts;

	fn snapshot() -> MetricsSnapshot {
		MetricsSnapshot {
			computed_at: jiff::Timestamp::from_second(1_690_000_000).unwrap(),
			counts: StatusCounts {
				passing: 30,
				warning: 2,
				failing: 1,
				skipped: 5,
				broken: 0,
			},
			stats: vec![
				(
					"sync_lookup",
					Stat::gauge("age_seconds", 12.0).help("Sync lookup staleness"),
				),
				("fhir_jobs", Stat::gauge("active_depth", 4.0)),
				(
					"fhir_jobs",
					Stat::gauge("jobs", 3.0).label("status", "Queued"),
				),
				(
					"fhir_jobs",
					Stat::gauge("jobs", 1.0).label("status", "Errored"),
				),
			],
		}
	}

	#[test]
	fn prometheus_census_and_families() {
		let out = render_prometheus(&snapshot());
		assert!(out.contains("bes_alertd_last_sweep_unix 1690000000"));
		assert!(out.contains("bes_alertd_checks{state=\"passing\"} 30"));
		assert!(out.contains("bes_alertd_checks{state=\"failing\"} 1"));
		// Scalar stat.
		assert!(out.contains("# TYPE bes_alertd_sync_lookup_age_seconds gauge"));
		assert!(out.contains("# HELP bes_alertd_sync_lookup_age_seconds Sync lookup staleness"));
		assert!(out.contains("bes_alertd_sync_lookup_age_seconds 12"));
		// Dimensioned stat: one family, two label series.
		assert!(out.contains("bes_alertd_fhir_jobs_jobs{status=\"Queued\"} 3"));
		assert!(out.contains("bes_alertd_fhir_jobs_jobs{status=\"Errored\"} 1"));
		// The family header appears exactly once for the labelled metric.
		assert_eq!(
			out.matches("# TYPE bes_alertd_fhir_jobs_jobs gauge")
				.count(),
			1
		);
	}

	#[test]
	fn munin_values() {
		let s = snapshot();
		let out = render_munin(Some(&s), 1_690_000_100, false);
		assert!(out.contains("multigraph bes_alertd_daemon"));
		assert!(out.contains("last_activity.value 1690000100"));
		assert!(out.contains("last_sweep.value 1690000000"));
		assert!(out.contains("multigraph bes_alertd_checks"));
		assert!(out.contains("passing.value 30"));
		assert!(out.contains("total.value 38"));
		assert!(out.contains("multigraph bes_alertd_fhir_jobs"));
		assert!(out.contains("active_depth.value 4"));
		// Labelled stat expands to one field per value.
		assert!(out.contains("jobs_queued.value 3"));
		assert!(out.contains("jobs_errored.value 1"));
	}

	#[test]
	fn munin_config() {
		let s = snapshot();
		let out = render_munin(Some(&s), 0, true);
		assert!(out.contains("multigraph bes_alertd_checks"));
		assert!(out.contains("graph_title Doctor checks by outcome"));
		assert!(out.contains("passing.type GAUGE"));
		assert!(out.contains("multigraph bes_alertd_fhir_jobs"));
		assert!(out.contains("graph_category bestool"));
		assert!(out.contains("jobs_queued.label Queued"));
		assert!(out.contains("jobs_queued.type GAUGE"));
		// No values in config mode.
		assert!(!out.contains(".value "));
	}

	#[test]
	fn munin_without_snapshot_is_liveness_only() {
		let values = render_munin(None, 42, false);
		assert!(values.contains("last_activity.value 42"));
		assert!(!values.contains("multigraph bes_alertd_checks"));
		assert!(!values.contains("last_sweep"));

		let config = render_munin(None, 0, true);
		assert!(config.contains("multigraph bes_alertd_daemon"));
		assert!(!config.contains("last_sweep"));
	}

	#[test]
	fn kind_is_respected() {
		let s = MetricsSnapshot {
			computed_at: jiff::Timestamp::from_second(0).unwrap(),
			counts: StatusCounts::default(),
			stats: vec![("http_errors", Stat::counter("requests_total", 9.0))],
		};
		assert!(
			render_prometheus(&s).contains("# TYPE bes_alertd_http_errors_requests_total counter")
		);
		assert!(render_munin(Some(&s), 0, true).contains("requests_total.type COUNTER"));
	}
}
