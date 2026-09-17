//! HTTP error rate over a sliding 10-minute window.
//!
//! The counters come from the substrate, which reads whatever fronts the
//! application. They are cumulative and only grow over a front end's lifetime,
//! so cumulative ratios become useless very quickly: a genuine spike right now
//! barely moves the needle against months of clean traffic.
//!
//! To get a rate that reflects *recent* health we snapshot the counters on every
//! sweep, then compare against the oldest snapshot that's still within the
//! window. With the default 1-minute cron there are normally ~10 snapshots
//! covering the last 10 minutes; ad-hoc manual runs piggy-back on whatever the
//! cron just wrote. If no usable historical snapshot exists (cold start, cache
//! wiped, front end restarted) we fall back to a 10-second in-run sample.
//!
//! History is kept per source, because the counters behind it are per front end
//! and those roll: a source that has vanished is dropped rather than its
//! disappearance being graded as the quantity having fallen.
//!
//! Only 5xx responses count as errors; 4xx responses are client mistakes
//! (bad URLs, auth, etc.) and aren't worth alerting on.
//!
//! spec: SUB#http-traffic-and-certificates

use std::{collections::BTreeMap, time::Duration};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::TamanuCx;
use crate::Stat;
use crate::check::Check;
use crate::runtime::{HttpRuntime, TrafficCounters, Unavailable};
use crate::store::{CheckStore, CheckStoreJson, Lifetime};

/// What this check remembers between sweeps, within its own subject's store.
const STATE_KEY: &str = "http_errors";

const WARN_ERROR_PCT: f64 = 5.0;
const FAIL_ERROR_PCT: f64 = 20.0;

/// How far back we'll compare current counters against. Older snapshots are
/// pruned.
const WINDOW: Duration = Duration::from_secs(10 * 60);
/// Grace beyond `WINDOW` before a snapshot is dropped from the history file.
const PRUNE_GRACE: Duration = Duration::from_secs(60);
/// Shortest usable historical window. If the freshest available history is
/// younger than this, do an in-run sample instead — a 5-second delta isn't a
/// rate, it's noise.
const MIN_HISTORY_AGE: Duration = Duration::from_secs(30);
/// Sleep between the two samples when we can't use history.
const IN_RUN_SAMPLE: Duration = Duration::from_secs(10);

/// One reading of every source's counters, as of a moment.
///
/// Keyed by source so a front end that rolls away takes its own counts with it
/// rather than looking like traffic that fell.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
	taken_at: Timestamp,
	#[serde(default)]
	sources: BTreeMap<String, BTreeMap<String, u64>>,
}

pub async fn run(ctx: TamanuCx) -> Check {
	let traffic = ctx.traffic.as_ref();
	let current = match sample(traffic).await {
		Ok(snapshot) => snapshot,
		Err(unavailable) => return unreadable(&unavailable),
	};

	let store = ctx.store.as_ref();
	let mut history: Vec<Snapshot> = store.get_json(STATE_KEY).await.unwrap_or_default();
	prune_history(&mut history, current.taken_at);

	let (baseline, source) = match pick_baseline(&history, &current) {
		Some(b) => (b.clone(), BaselineSource::History),
		None => {
			tokio::time::sleep(IN_RUN_SAMPLE).await;
			let second = match sample(traffic).await {
				Ok(snapshot) => snapshot,
				Err(unavailable) => return unreadable(&unavailable),
			};
			// `current` was taken first; second was taken IN_RUN_SAMPLE later.
			// Re-assign so `current` is the newer one for the delta math below.
			let baseline = current.clone();
			append_and_save(store, &mut history, second.clone()).await;
			return build_check(&baseline, &second, BaselineSource::InRunSample);
		}
	};

	append_and_save(store, &mut history, current.clone()).await;
	build_check(&baseline, &current, source)
}

/// One reading of every source's counters, now.
async fn sample(traffic: &dyn HttpRuntime) -> Result<Snapshot, Unavailable> {
	traffic
		.http_counters()
		.await
		.map(|counters| snapshot_of(counters, Timestamp::now()))
}

/// The skip for a reading the substrate could not take, carrying its reason
/// rather than one of this check's own.
fn unreadable(unavailable: &Unavailable) -> Check {
	Check::skip(
		"http_errors",
		"traffic could not be read",
		unavailable.reason(),
	)
}

fn snapshot_of(counters: TrafficCounters, taken_at: Timestamp) -> Snapshot {
	Snapshot {
		taken_at,
		sources: counters
			.sources
			.into_iter()
			.map(|source| (source.source, source.by_status))
			.collect(),
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BaselineSource {
	History,
	InRunSample,
}

fn pick_baseline<'a>(history: &'a [Snapshot], current: &Snapshot) -> Option<&'a Snapshot> {
	history
		.iter()
		.filter(|s| {
			let age = duration_between(s.taken_at, current.taken_at);
			age >= MIN_HISTORY_AGE && age <= WINDOW
		})
		// A counter going down means the source restarted between the snapshots
		// and the delta would be meaningless. Skip such baselines.
		.filter(|s| !counters_reset(s, current))
		// Oldest still-usable snapshot gives the widest window.
		.min_by_key(|s| s.taken_at)
}

/// Whether a source's own counters went backwards, which means it restarted
/// between the two readings and the delta would be meaningless.
///
/// Asked per source: a source present in both readings is compared, and one
/// that has gone is an absence rather than a decrease, so it says nothing about
/// whether the sources that remain restarted.
fn counters_reset(before: &Snapshot, after: &Snapshot) -> bool {
	before.sources.iter().any(|(source, was)| {
		let Some(now) = after.sources.get(source) else {
			return false;
		};
		was.iter()
			.any(|(code, b)| now.get(code).copied().unwrap_or(0) < *b)
	})
}

/// What each status code gained between the two readings, summed over the
/// sources present in both.
///
/// A source only in the later reading contributes its whole count, because it
/// started within the window. A source only in the earlier one is dropped: it
/// has gone, and subtracting its last count would grade its disappearance as
/// the quantity having fallen.
fn delta_counts(before: &Snapshot, after: &Snapshot) -> BTreeMap<String, u64> {
	let mut out = BTreeMap::new();
	for (source, now) in &after.sources {
		let was = before.sources.get(source);
		for (code, after_n) in now {
			let before_n = was.and_then(|was| was.get(code)).copied().unwrap_or(0);
			let d = after_n.saturating_sub(before_n);
			if d > 0 {
				*out.entry(code.clone()).or_insert(0) += d;
			}
		}
	}
	out
}

/// Every status code counted right now, summed across sources. What the traffic
/// telemetry reports, as against the windowed delta the rate is graded on.
fn total_counts(snapshot: &Snapshot) -> BTreeMap<String, u64> {
	let mut out = BTreeMap::new();
	for counts in snapshot.sources.values() {
		for (code, n) in counts {
			*out.entry(code.clone()).or_insert(0) += n;
		}
	}
	out
}

fn build_check(baseline: &Snapshot, current: &Snapshot, source: BaselineSource) -> Check {
	let deltas = delta_counts(baseline, current);
	let total: u64 = deltas.values().sum();
	let errored: u64 = deltas
		.iter()
		.filter(|(code, _)| code.starts_with('5'))
		.map(|(_, n)| n)
		.sum();
	let window = duration_between(baseline.taken_at, current.taken_at);
	let window_label = humanise_window(window);
	let source_label = match source {
		BaselineSource::History => "vs history",
		BaselineSource::InRunSample => "live sample",
	};

	if total == 0 {
		return with_traffic_stats(
			Check::pass(
				"http_errors",
				format!("no requests in last {window_label} ({source_label})"),
			)
			.with_detail("total_requests", 0u64)
			.with_detail("window_seconds", window.as_secs())
			.with_detail("baseline_source", source_label),
			&total_counts(current),
		);
	}

	let pct = ((errored as f64 / total as f64) * 100.0).round();
	let summary = format!(
		"{errored}/{total} server errors ({pct:.0}%) in last {window_label} ({source_label})"
	);

	let check = if pct >= FAIL_ERROR_PCT {
		Check::fail(
			"http_errors",
			summary.clone(),
			format!("≥{FAIL_ERROR_PCT}% error rate"),
		)
	} else if pct >= WARN_ERROR_PCT {
		Check::warning(
			"http_errors",
			summary.clone(),
			format!("≥{WARN_ERROR_PCT}% error rate"),
		)
	} else {
		Check::pass("http_errors", summary)
	};

	let mut by_code: Map<String, Value> = Map::new();
	for (code, n) in &deltas {
		by_code.insert(code.clone(), Value::from(*n));
	}

	with_traffic_stats(
		check
			.with_detail("total_requests", total)
			.with_detail("server_error_requests", errored)
			.with_detail("server_error_rate_pct", pct)
			.with_detail("window_seconds", window.as_secs())
			.with_detail("baseline_source", source_label)
			.with_detail("by_code", Value::Object(by_code)),
		&total_counts(current),
	)
	.with_stat(
		Stat::gauge("server_error_rate_pct", pct)
			.namespace("http")
			.help("5xx rate, percent"),
	)
}

/// Attach Caddy's cumulative request counters to a check.
///
/// The verdict above comes from a delta over a window whose length varies with
/// what history is on disk, but a metric that carried that window would be
/// uninterpretable without it. So the published metrics are the raw cumulative
/// totals and a scrape derives its own rate over its own interval; the window
/// stays a fact reported to canopy.
fn with_traffic_stats(check: Check, counts: &BTreeMap<String, u64>) -> Check {
	let total: u64 = counts.values().sum();
	let errored: u64 = counts
		.iter()
		.filter(|(code, _)| code.starts_with('5'))
		.map(|(_, n)| n)
		.sum();

	check
		.with_stat(
			Stat::counter("requests_total", total as f64)
				.namespace("http")
				.group("traffic")
				.help("Requests served"),
		)
		.with_stat(
			Stat::counter("server_errors_total", errored as f64)
				.namespace("http")
				.group("traffic")
				.help("5xx responses"),
		)
		.with_stats(counts.iter().map(|(code, n)| {
			Stat::counter("requests_by_code_total", *n as f64)
				.namespace("http")
				.label("code", code.clone())
				.help("Requests served by HTTP status code")
		}))
}

fn duration_between(earlier: Timestamp, later: Timestamp) -> Duration {
	let secs = later.as_second().saturating_sub(earlier.as_second());
	Duration::from_secs(secs.max(0) as u64)
}

fn humanise_window(d: Duration) -> String {
	let secs = d.as_secs();
	if secs < 60 {
		format!("{secs}s")
	} else {
		let m = secs / 60;
		let s = secs % 60;
		if s == 0 {
			format!("{m}m")
		} else {
			format!("{m}m {s}s")
		}
	}
}

fn prune_history(history: &mut Vec<Snapshot>, now: Timestamp) {
	let cutoff = WINDOW + PRUNE_GRACE;
	history.retain(|s| {
		let age = duration_between(s.taken_at, now);
		age <= cutoff
	});
}

/// Keep the new snapshot, and write the pruned history back.
///
/// Stored as lasting only until the compute restarts: these are a front end's
/// counters, which start again from zero when it does, so a baseline retained
/// across that would read the fresh counters as a reset or as a plausible delta.
async fn append_and_save(store: &dyn CheckStore, history: &mut Vec<Snapshot>, snapshot: Snapshot) {
	history.push(snapshot);
	store
		.put_json(STATE_KEY, &*history, Lifetime::UntilCompute)
		.await;
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A reading from one source, which is what a machine's own front end
	/// gives.
	fn snap(secs: i64, counts: &[(&str, u64)]) -> Snapshot {
		snap_of(secs, &[("caddy", counts)])
	}

	/// A reading from several sources, as shared infrastructure gives.
	fn snap_of(secs: i64, sources: &[(&str, &[(&str, u64)])]) -> Snapshot {
		Snapshot {
			taken_at: Timestamp::from_second(secs).unwrap(),
			sources: sources
				.iter()
				.map(|(source, counts)| {
					(
						(*source).to_string(),
						counts.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
					)
				})
				.collect(),
		}
	}

	/// A source that has gone is an absence, not a decrease: subtracting its
	/// last count would grade a rolled-away front end as traffic that fell.
	///
	/// spec: SUB#http-traffic-and-certificates
	#[test]
	fn a_vanished_source_is_dropped_rather_than_counted_down() {
		let before = snap_of(
			0,
			&[
				("pod-a", &[("200", 100)][..]),
				("pod-b", &[("200", 100)][..]),
			],
		);
		let after = snap_of(60, &[("pod-b", &[("200", 150)][..])]);

		assert!(
			!counters_reset(&before, &after),
			"pod-a going away is not pod-b's counters going backwards"
		);
		assert_eq!(
			delta_counts(&before, &after).get("200").copied(),
			Some(50),
			"only pod-b's own gain counts"
		);
	}

	/// A source that appeared within the window contributes everything it has
	/// counted, because it started inside the window.
	#[test]
	fn a_new_source_contributes_its_whole_count() {
		let before = snap_of(0, &[("pod-a", &[("200", 100)][..])]);
		let after = snap_of(
			60,
			&[("pod-a", &[("200", 110)][..]), ("pod-b", &[("200", 7)][..])],
		);
		assert_eq!(delta_counts(&before, &after).get("200").copied(), Some(17));
	}

	/// A source whose own counters went backwards restarted, so the delta
	/// against it would be meaningless and the baseline is not usable.
	#[test]
	fn a_restarted_source_invalidates_the_baseline() {
		let before = snap_of(0, &[("pod-a", &[("200", 100)][..])]);
		let after = snap_of(60, &[("pod-a", &[("200", 3)][..])]);
		assert!(counters_reset(&before, &after));
	}

	#[test]
	fn build_check_emits_cumulative_counters() {
		use crate::StatKind;

		let baseline = snap(0, &[("200", 100), ("500", 0), ("502", 0)]);
		let current = snap(60, &[("200", 190), ("500", 5), ("502", 5)]);
		let check = build_check(&baseline, &current, BaselineSource::History);

		let stat = |name: &str| check.stats.iter().find(|s| s.name == name).expect(name);
		// The window delta is 90 + 5 + 5 requests; the metrics are Caddy's totals.
		assert_eq!(stat("requests_total").value, 200.0);
		assert_eq!(stat("server_errors_total").value, 10.0);
		assert_eq!(stat("requests_total").kind, StatKind::Counter);
		assert_eq!(stat("server_errors_total").kind, StatKind::Counter);

		// the verdict's own number stays a percentage over the window
		assert_eq!(stat("server_error_rate_pct").value, 10.0);

		// dimensioned by-code stats carry the code label, and are cumulative too
		let by_code: Vec<_> = check
			.stats
			.iter()
			.filter(|s| s.name == "requests_by_code_total")
			.collect();
		assert!(
			by_code
				.iter()
				.any(|s| { s.labels == vec![("code", "200".to_string())] && s.value == 190.0 })
		);
		assert!(by_code.iter().all(|s| s.kind == StatKind::Counter));
	}

	#[test]
	fn quiet_window_still_publishes_totals() {
		// A window with no traffic doesn't reset Caddy's counters, so the totals
		// keep reporting where they are rather than dropping to zero.
		let counts: &[(&str, u64)] = &[("200", 4200), ("502", 7)];
		let check = build_check(
			&snap(0, counts),
			&snap(600, counts),
			BaselineSource::History,
		);

		let scalar = |name: &str| check.stats.iter().find(|s| s.name == name).map(|s| s.value);
		assert_eq!(scalar("requests_total"), Some(4207.0));
		assert_eq!(scalar("server_errors_total"), Some(7.0));
		// with no requests in the window there is no error rate to report
		assert_eq!(scalar("server_error_rate_pct"), None);
	}

	#[test]
	fn delta_only_counts_growth() {
		let before = snap(0, &[("200", 10), ("500", 2)]);
		let after = snap(60, &[("200", 15), ("500", 4), ("404", 1)]);
		let d = delta_counts(&before, &after);
		assert_eq!(d.get("200").copied(), Some(5));
		assert_eq!(d.get("500").copied(), Some(2));
		assert_eq!(d.get("404").copied(), Some(1));
	}

	#[test]
	fn reset_detected_when_any_counter_drops() {
		let before = snap(0, &[("200", 10)]);
		assert!(counters_reset(&before, &snap(60, &[("200", 5)])));
		assert!(!counters_reset(&before, &snap(60, &[("200", 11)])));
	}

	#[test]
	fn pick_baseline_prefers_oldest_within_window() {
		let now = Timestamp::from_second(10_000).unwrap();
		let current = snap(now.as_second(), &[("200", 100)]);
		let history = vec![
			snap(10_000 - 700, &[("200", 10)]), // 11m40s old — too old
			snap(10_000 - 540, &[("200", 30)]), // 9m old — usable
			snap(10_000 - 300, &[("200", 60)]), // 5m old — usable
			snap(10_000 - 10, &[("200", 90)]),  // 10s old — too fresh
		];
		let baseline = pick_baseline(&history, &current).expect("should pick one");
		assert_eq!(baseline.taken_at.as_second(), 10_000 - 540);
	}

	#[test]
	fn pick_baseline_skips_when_only_fresh_snapshots() {
		let now = Timestamp::from_second(10_000).unwrap();
		let current = snap(now.as_second(), &[("200", 100)]);
		let history = vec![snap(10_000 - 5, &[("200", 95)])];
		assert!(pick_baseline(&history, &current).is_none());
	}

	#[test]
	fn pick_baseline_skips_resets() {
		let now = Timestamp::from_second(10_000).unwrap();
		let current = snap(now.as_second(), &[("200", 5)]);
		let history = vec![snap(10_000 - 300, &[("200", 100)])];
		assert!(pick_baseline(&history, &current).is_none());
	}

	#[test]
	fn prune_drops_snapshots_outside_window_plus_grace() {
		let now = Timestamp::from_second(10_000).unwrap();
		let mut history = vec![
			snap(
				10_000 - (WINDOW + PRUNE_GRACE).as_secs() as i64 - 1,
				&[("200", 1)],
			),
			snap(10_000 - WINDOW.as_secs() as i64, &[("200", 2)]),
			snap(10_000 - 60, &[("200", 3)]),
		];
		prune_history(&mut history, now);
		assert_eq!(history.len(), 2);
		assert_eq!(history[0].sources["caddy"].get("200").copied(), Some(2));
	}

	#[test]
	fn humanise_window_formats_seconds_and_minutes() {
		assert_eq!(humanise_window(Duration::from_secs(10)), "10s");
		assert_eq!(humanise_window(Duration::from_secs(60)), "1m");
		assert_eq!(humanise_window(Duration::from_secs(540)), "9m");
		assert_eq!(humanise_window(Duration::from_secs(545)), "9m 5s");
	}
}
