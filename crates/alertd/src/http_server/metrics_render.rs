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

/// Render the prometheus body: daemon liveness (as an age), and — when a sweep
/// is cached — the check census and per-check stats. `now` and `last_activity`
/// are unix seconds; the liveness age is their difference, computed here so a
/// scraper reads seconds-since directly.
pub fn render_prometheus(
	snapshot: Option<&MetricsSnapshot>,
	now: i64,
	last_activity: i64,
) -> String {
	let mut out = String::new();

	let _ = writeln!(
		out,
		"# HELP {PREFIX}_last_activity_age_seconds Seconds since the daemon was last active"
	);
	let _ = writeln!(out, "# TYPE {PREFIX}_last_activity_age_seconds gauge");
	let _ = writeln!(
		out,
		"{PREFIX}_last_activity_age_seconds {}",
		now - last_activity
	);

	let Some(snapshot) = snapshot else {
		return out;
	};

	let _ = writeln!(
		out,
		"# HELP {PREFIX}_last_sweep_age_seconds Seconds since the last doctor sweep"
	);
	let _ = writeln!(out, "# TYPE {PREFIX}_last_sweep_age_seconds gauge");
	let _ = writeln!(
		out,
		"{PREFIX}_last_sweep_age_seconds {}",
		now - snapshot.computed_at.as_second()
	);

	let _ = writeln!(
		out,
		"# HELP {PREFIX}_checks Number of doctor checks by outcome"
	);
	let _ = writeln!(out, "# TYPE {PREFIX}_checks gauge");
	for (state, count) in snapshot.counts.by_state() {
		let _ = writeln!(out, "{PREFIX}_checks{{state=\"{state}\"}} {count}");
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
	now: i64,
	last_activity: i64,
	config: bool,
) -> String {
	let mut out = String::new();

	// Daemon graph: how long ago the daemon was last active and last swept, as
	// ages in seconds rather than raw timestamps (which graph as an ever-rising
	// line). `now` and the instants are unix seconds.
	let _ = writeln!(out, "multigraph bes_alertd_daemon");
	if config {
		let _ = writeln!(out, "graph_title alertd daemon activity");
		let _ = writeln!(out, "graph_category bestool");
		let _ = writeln!(out, "graph_vlabel seconds ago");
		let _ = writeln!(out, "last_activity.label last activity (seconds ago)");
		let _ = writeln!(out, "last_activity.type GAUGE");
		if snapshot.is_some() {
			let _ = writeln!(out, "last_sweep.label last sweep (seconds ago)");
			let _ = writeln!(out, "last_sweep.type GAUGE");
		}
	} else {
		let _ = writeln!(out, "last_activity.value {}", now - last_activity);
		if let Some(s) = snapshot {
			let _ = writeln!(out, "last_sweep.value {}", now - s.computed_at.as_second());
		}
	}

	let Some(snapshot) = snapshot else {
		return out;
	};

	// Census graph.
	let _ = writeln!(out, "\nmultigraph bes_alertd_checks");
	if config {
		let _ = writeln!(out, "graph_title Doctor checks by outcome");
		let _ = writeln!(out, "graph_category bestool");
		let _ = writeln!(out, "graph_vlabel checks");
		// Stack the outcome counts as areas so the make-up reads at a glance;
		// the total rides over them as a line rather than adding to the stack.
		let _ = writeln!(out, "graph_args --lower-limit 0");
		for state in STATE_NAMES {
			let _ = writeln!(out, "{state}.label {state}");
			let _ = writeln!(out, "{state}.type GAUGE");
			let _ = writeln!(out, "{state}.draw AREASTACK");
		}
		let _ = writeln!(out, "total.label total");
		let _ = writeln!(out, "total.type GAUGE");
		let _ = writeln!(out, "total.draw LINE1");
	} else {
		for (state, count) in snapshot.counts.by_state() {
			let _ = writeln!(out, "{state}.value {count}");
		}
		let _ = writeln!(out, "total.value {}", snapshot.counts.total());
	}

	// One multigraph per (check, group), in first-seen order — a metric with no
	// explicit group forms its own graph. A check's unrelated metrics — a count,
	// a percentage, a duration, a per-code breakdown — would otherwise share a
	// single graph's Y axis, which is meaningless; and metrics that differ only
	// by name while sharing a label value (e.g. several btrfs gauges all tagged
	// `mount=/`) would render as indistinguishable same-labelled fields. A check
	// groups metrics that share a unit and are read together; label-dimensioned
	// series of one metric become the fields of its graph.
	let mut order: Vec<(&str, &str)> = Vec::new();
	let mut by_group: std::collections::HashMap<(&str, &str), Vec<&Stat>> =
		std::collections::HashMap::new();
	for (check, stat) in &snapshot.stats {
		let check: &str = check;
		// A metric with no explicit group forms its own graph, named for it.
		let group = stat.group.unwrap_or(stat.name);
		by_group
			.entry((check, group))
			.or_insert_with(|| {
				order.push((check, group));
				Vec::new()
			})
			.push(stat);
	}

	for (check, group) in order {
		let _ = writeln!(
			out,
			"\nmultigraph {PREFIX}_{check}_{}",
			munin_field_segment(group)
		);
		let stats = &by_group[&(check, group)];
		if config {
			let _ = writeln!(out, "graph_title {check} {group}");
			let _ = writeln!(out, "graph_category bestool");
			// The metrics of one group share a description; use it as the graph's.
			if let Some(help) = stats.iter().find_map(|s| s.help.as_deref()) {
				let _ = writeln!(out, "graph_info {}", help.replace('\n', " "));
			}
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
	fn prometheus_liveness_census_and_families() {
		// now is 50s after both the last activity and the sweep.
		let out = render_prometheus(Some(&snapshot()), 1_690_000_050, 1_690_000_000);
		// Liveness is reported as an age, not a raw timestamp.
		assert!(out.contains("bes_alertd_last_activity_age_seconds 50"));
		assert!(out.contains("bes_alertd_last_sweep_age_seconds 50"));
		assert!(!out.contains("_unix"));
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
		// now is 10s after the last activity and 100s after the sweep.
		let out = render_munin(Some(&s), 1_690_000_100, 1_690_000_090, false);
		assert!(out.contains("multigraph bes_alertd_daemon"));
		// Liveness values are ages, not raw timestamps.
		assert!(out.contains("last_activity.value 10"));
		assert!(out.contains("last_sweep.value 100"));
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
		let out = render_munin(Some(&s), 0, 0, true);
		assert!(out.contains("multigraph bes_alertd_checks"));
		assert!(out.contains("graph_title Doctor checks by outcome"));
		// The census is a stacked area with the total overlaid as a line.
		assert!(out.contains("passing.draw AREASTACK"));
		assert!(out.contains("total.draw LINE1"));
		assert!(out.contains("passing.type GAUGE"));
		assert!(out.contains("multigraph bes_alertd_fhir_jobs"));
		assert!(out.contains("graph_category bestool"));
		assert!(out.contains("jobs_queued.label Queued"));
		assert!(out.contains("jobs_queued.type GAUGE"));
		// No values in config mode.
		assert!(!out.contains(".value "));
	}

	#[test]
	fn munin_splits_a_check_into_per_name_graphs() {
		// Two metrics of one check that share a label value must not collapse
		// into one graph of indistinguishable fields — each metric name gets
		// its own graph, titled distinctly.
		let s = MetricsSnapshot {
			computed_at: jiff::Timestamp::from_second(0).unwrap(),
			counts: StatusCounts::default(),
			stats: vec![
				(
					"btrfs",
					Stat::gauge("device_unallocated_bytes", 1.0)
						.label("mount", "/")
						.help("Unallocated btrfs space"),
				),
				(
					"btrfs",
					Stat::gauge("metadata_percent", 2.0)
						.label("mount", "/")
						.help("btrfs metadata chunk usage, percent"),
				),
			],
		};
		let out = render_munin(Some(&s), 0, 0, true);
		assert!(out.contains("multigraph bes_alertd_btrfs_device_unallocated_bytes"));
		assert!(out.contains("multigraph bes_alertd_btrfs_metadata_percent"));
		assert!(out.contains("graph_title btrfs device_unallocated_bytes"));
		assert!(out.contains("graph_title btrfs metadata_percent"));
	}

	#[test]
	fn munin_can_group_metrics_of_a_check() {
		// Two metrics sharing an explicit group render as fields of one graph.
		let s = MetricsSnapshot {
			computed_at: jiff::Timestamp::from_second(0).unwrap(),
			counts: StatusCounts::default(),
			stats: vec![
				(
					"memory",
					Stat::gauge("used_bytes", 1.0)
						.group("bytes")
						.help("Memory in use"),
				),
				(
					"memory",
					Stat::gauge("total_bytes", 2.0)
						.group("bytes")
						.help("Total memory"),
				),
			],
		};
		let out = render_munin(Some(&s), 0, 0, true);
		assert!(out.contains("multigraph bes_alertd_memory_bytes"));
		assert!(!out.contains("multigraph bes_alertd_memory_used_bytes"));
		assert!(out.contains("used_bytes.label Memory in use"));
		assert!(out.contains("total_bytes.label Total memory"));
	}

	#[test]
	fn munin_without_snapshot_is_liveness_only() {
		// now is 2s after the last activity.
		let values = render_munin(None, 44, 42, false);
		assert!(values.contains("last_activity.value 2"));
		assert!(!values.contains("multigraph bes_alertd_checks"));
		assert!(!values.contains("last_sweep"));

		let config = render_munin(None, 0, 0, true);
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
			render_prometheus(Some(&s), 0, 0)
				.contains("# TYPE bes_alertd_http_errors_requests_total counter")
		);
		assert!(render_munin(Some(&s), 0, 0, true).contains("requests_total.type COUNTER"));
	}
}
