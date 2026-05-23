//! HTTP error rate over a sliding 10-minute window.
//!
//! Caddy's admin API at `localhost:2019` exposes `/metrics` in Prometheus text
//! format. The relevant series is `caddy_http_request_duration_seconds_count`,
//! labelled with the HTTP status code. Prometheus counters only grow over
//! Caddy's lifetime, so cumulative ratios become useless very quickly: a
//! genuine spike right now barely moves the needle against months of clean
//! traffic.
//!
//! To get a rate that reflects *recent* health we snapshot the counters to
//! disk on every doctor run, then compare against the oldest snapshot that's
//! still within the window. With the default 1-minute cron there are normally
//! ~10 snapshots covering the last 10 minutes; ad-hoc manual runs piggy-back
//! on whatever the cron just wrote. If no usable historical snapshot exists
//! (cold start, cache wiped, Caddy restarted) we fall back to a 10-second
//! in-run sample.
//!
//! Only 5xx responses count as errors; 4xx responses are client mistakes
//! (bad URLs, auth, etc.) and aren't worth alerting on.

use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
	time::Duration,
};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tracing::{debug, warn};

use super::CheckContext;
use crate::doctor::check::Check;

const CADDY_METRICS_URL: &str = "http://localhost:2019/metrics";
const TIMEOUT: Duration = Duration::from_secs(3);

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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
	taken_at: Timestamp,
	counts: BTreeMap<String, u64>,
}

pub async fn run(_ctx: CheckContext) -> Check {
	let client = match reqwest::Client::builder().timeout(TIMEOUT).build() {
		Ok(c) => c,
		Err(err) => {
			return Check::warning(
				"http_errors",
				"could not build HTTP client",
				err.to_string(),
			)
			.with_detail("skipped", true);
		}
	};

	let current_counts = match fetch_counts(&client).await {
		FetchResult::Counts(c) => c,
		FetchResult::Skip(check) => return check,
	};
	let current = Snapshot {
		taken_at: Timestamp::now(),
		counts: current_counts,
	};

	let state = state_path();
	let mut history = state.as_deref().map(load_history).unwrap_or_default();
	prune_history(&mut history, current.taken_at);

	let (baseline, source) = match pick_baseline(&history, &current) {
		Some(b) => (b.clone(), BaselineSource::History),
		None => {
			tokio::time::sleep(IN_RUN_SAMPLE).await;
			let second_counts = match fetch_counts(&client).await {
				FetchResult::Counts(c) => c,
				FetchResult::Skip(check) => return check,
			};
			let second = Snapshot {
				taken_at: Timestamp::now(),
				counts: second_counts,
			};
			// `current` was taken first; second was taken IN_RUN_SAMPLE later.
			// Re-assign so `current` is the newer one for the delta math below.
			let baseline = current.clone();
			append_and_save(&state, &mut history, second.clone());
			return build_check(&baseline, &second, BaselineSource::InRunSample);
		}
	};

	append_and_save(&state, &mut history, current.clone());
	build_check(&baseline, &current, source)
}

enum FetchResult {
	Counts(BTreeMap<String, u64>),
	Skip(Check),
}

async fn fetch_counts(client: &reqwest::Client) -> FetchResult {
	let body = match client.get(CADDY_METRICS_URL).send().await {
		Ok(resp) if resp.status().is_success() => match resp.text().await {
			Ok(t) => t,
			Err(err) => {
				return FetchResult::Skip(
					Check::pass("http_errors", "caddy /metrics body read failed")
						.with_detail("skipped", true)
						.with_detail("reason", err.to_string()),
				);
			}
		},
		Ok(resp) => {
			return FetchResult::Skip(
				Check::pass(
					"http_errors",
					format!("caddy /metrics returned HTTP {}", resp.status().as_u16()),
				)
				.with_detail("skipped", true),
			);
		}
		Err(_) => {
			return FetchResult::Skip(
				Check::pass("http_errors", "caddy admin unreachable").with_detail("skipped", true),
			);
		}
	};

	FetchResult::Counts(parse_status_counts(&body))
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
		// A counter going down means Caddy restarted between the snapshots and
		// the delta would be meaningless. Skip such baselines.
		.filter(|s| !counters_reset(&s.counts, &current.counts))
		// Oldest still-usable snapshot gives the widest window.
		.min_by_key(|s| s.taken_at)
}

fn counters_reset(before: &BTreeMap<String, u64>, after: &BTreeMap<String, u64>) -> bool {
	before
		.iter()
		.any(|(code, b)| after.get(code).copied().unwrap_or(0) < *b)
}

fn delta_counts(
	before: &BTreeMap<String, u64>,
	after: &BTreeMap<String, u64>,
) -> BTreeMap<String, u64> {
	let mut out = BTreeMap::new();
	for (code, after_n) in after {
		let before_n = before.get(code).copied().unwrap_or(0);
		let d = after_n.saturating_sub(before_n);
		if d > 0 {
			out.insert(code.clone(), d);
		}
	}
	out
}

fn build_check(baseline: &Snapshot, current: &Snapshot, source: BaselineSource) -> Check {
	let deltas = delta_counts(&baseline.counts, &current.counts);
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
		return Check::pass(
			"http_errors",
			format!("no requests in last {window_label} ({source_label})"),
		)
		.with_detail("total_requests", 0u64)
		.with_detail("window_seconds", window.as_secs())
		.with_detail("baseline_source", source_label);
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

	check
		.with_detail("total_requests", total)
		.with_detail("server_error_requests", errored)
		.with_detail("server_error_rate_pct", pct)
		.with_detail("window_seconds", window.as_secs())
		.with_detail("baseline_source", source_label)
		.with_detail("by_code", Value::Object(by_code))
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

fn state_path() -> Option<PathBuf> {
	dirs::cache_dir().map(|d| d.join("bestool").join("doctor-http-errors.json"))
}

fn load_history(path: &Path) -> Vec<Snapshot> {
	match std::fs::read(path) {
		Ok(bytes) => match serde_json::from_slice::<Vec<Snapshot>>(&bytes) {
			Ok(v) => v,
			Err(err) => {
				debug!(%err, ?path, "ignoring unparseable doctor http_errors history");
				Vec::new()
			}
		},
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
		Err(err) => {
			debug!(%err, ?path, "could not read doctor http_errors history");
			Vec::new()
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

fn append_and_save(path: &Option<PathBuf>, history: &mut Vec<Snapshot>, snapshot: Snapshot) {
	history.push(snapshot);
	let Some(path) = path else { return };
	if let Some(parent) = path.parent()
		&& let Err(err) = std::fs::create_dir_all(parent)
	{
		warn!(%err, ?parent, "could not create doctor http_errors cache dir");
		return;
	}
	let json = match serde_json::to_vec(history) {
		Ok(b) => b,
		Err(err) => {
			warn!(%err, "could not serialise doctor http_errors history");
			return;
		}
	};
	let tmp = path.with_extension("json.tmp");
	if let Err(err) = std::fs::write(&tmp, &json) {
		warn!(%err, ?tmp, "could not write doctor http_errors history");
		return;
	}
	if let Err(err) = std::fs::rename(&tmp, path) {
		warn!(%err, ?path, "could not rename doctor http_errors history");
	}
}

/// Parse `caddy_http_request_duration_seconds_count{code="NNN",...} <count>` lines.
///
/// Caddy emits this histogram-count series labelled by `code`, `handler`,
/// `host`, `method`, `server`. The same request is observed by every handler
/// in the chain (encode, headers, rate_limit, reverse_proxy, …), so a naive
/// sum across labels would multiply the real request count by the depth of
/// the handler chain. To dedupe, we group by `(host, method, server, code)`
/// and take the **max** across handlers: the entry-point handler must have
/// seen every request matching that label combination, so its count is the
/// real one. Then we sum across hosts/methods/servers per code.
fn parse_status_counts(body: &str) -> BTreeMap<String, u64> {
	use std::collections::HashMap;

	let mut per_tuple: HashMap<(String, String, String, String), u64> = HashMap::new();
	for line in body.lines() {
		if line.starts_with('#') {
			continue;
		}
		let Some(rest) = line.strip_prefix("caddy_http_request_duration_seconds_count") else {
			continue;
		};
		let Some(labels_end) = rest.find('}') else {
			continue;
		};
		let labels = &rest[..labels_end];
		let value_part = rest[labels_end + 1..].trim();
		let value: u64 = match value_part.split_whitespace().next() {
			Some(v) => match v.parse::<f64>() {
				Ok(f) => f as u64,
				Err(_) => continue,
			},
			None => continue,
		};
		let Some(code) = extract_label(labels, "code") else {
			continue;
		};
		let host = extract_label(labels, "host").unwrap_or_default();
		let method = extract_label(labels, "method").unwrap_or_default();
		let server = extract_label(labels, "server").unwrap_or_default();
		let key = (host, method, server, code);
		let entry = per_tuple.entry(key).or_insert(0);
		*entry = (*entry).max(value);
	}

	let mut totals: BTreeMap<String, u64> = BTreeMap::new();
	for ((_, _, _, code), count) in per_tuple {
		*totals.entry(code).or_insert(0) += count;
	}
	totals
}

fn extract_label(labels: &str, key: &str) -> Option<String> {
	let needle = format!("{key}=\"");
	let start = labels.find(&needle)? + needle.len();
	let rest = &labels[start..];
	let end = rest.find('"')?;
	Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	const SAMPLE: &str = "\
# HELP caddy_http_request_duration_seconds Histogram of round-trip request durations.
# TYPE caddy_http_request_duration_seconds histogram
caddy_http_request_duration_seconds_count{code=\"200\",handler=\"encode\",host=\"a\",method=\"GET\",server=\"srv0\"} 3
caddy_http_request_duration_seconds_count{code=\"200\",handler=\"headers\",host=\"a\",method=\"GET\",server=\"srv0\"} 9
caddy_http_request_duration_seconds_count{code=\"200\",handler=\"rate_limit\",host=\"a\",method=\"GET\",server=\"srv0\"} 3
caddy_http_request_duration_seconds_count{code=\"200\",handler=\"reverse_proxy\",host=\"a\",method=\"GET\",server=\"srv0\"} 3
caddy_http_request_duration_seconds_count{code=\"404\",handler=\"headers\",host=\"a\",method=\"GET\",server=\"srv0\"} 12
caddy_http_request_duration_seconds_count{code=\"502\",handler=\"reverse_proxy\",host=\"a\",method=\"POST\",server=\"srv0\"} 3
caddy_http_request_duration_seconds_bucket{code=\"200\",handler=\"encode\",host=\"a\",method=\"GET\",server=\"srv0\",le=\"0.005\"} 3
other_metric{foo=\"bar\"} 7
";

	fn snap(secs: i64, counts: &[(&str, u64)]) -> Snapshot {
		Snapshot {
			taken_at: Timestamp::from_second(secs).unwrap(),
			counts: counts.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
		}
	}

	#[test]
	fn parses_caddy_metric_lines() {
		let counts = parse_status_counts(SAMPLE);
		assert_eq!(
			counts.into_iter().collect::<Vec<_>>(),
			vec![
				("200".to_string(), 9),
				("404".to_string(), 12),
				("502".to_string(), 3),
			]
		);
	}

	#[test]
	fn ignores_unrelated_metrics() {
		let counts = parse_status_counts("foo_bar{code=\"500\"} 99");
		assert!(counts.is_empty());
	}

	#[test]
	fn label_extract_simple() {
		assert_eq!(
			extract_label("{code=\"200\",server=\"srv0\"}", "code"),
			Some("200".to_string())
		);
	}

	#[test]
	fn delta_only_counts_growth() {
		let before: BTreeMap<String, u64> =
			[("200".to_string(), 10), ("500".to_string(), 2)].into();
		let after: BTreeMap<String, u64> = [
			("200".to_string(), 15),
			("500".to_string(), 4),
			("404".to_string(), 1),
		]
		.into();
		let d = delta_counts(&before, &after);
		assert_eq!(d.get("200").copied(), Some(5));
		assert_eq!(d.get("500").copied(), Some(2));
		assert_eq!(d.get("404").copied(), Some(1));
	}

	#[test]
	fn reset_detected_when_any_counter_drops() {
		let before: BTreeMap<String, u64> = [("200".to_string(), 10)].into();
		let after_dropped: BTreeMap<String, u64> = [("200".to_string(), 5)].into();
		assert!(counters_reset(&before, &after_dropped));
		let after_grown: BTreeMap<String, u64> = [("200".to_string(), 11)].into();
		assert!(!counters_reset(&before, &after_grown));
	}

	#[test]
	fn pick_baseline_prefers_oldest_within_window() {
		let now = Timestamp::from_second(10_000).unwrap();
		let current = Snapshot {
			taken_at: now,
			counts: [("200".to_string(), 100)].into(),
		};
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
		let current = Snapshot {
			taken_at: now,
			counts: [("200".to_string(), 100)].into(),
		};
		let history = vec![snap(10_000 - 5, &[("200", 95)])];
		assert!(pick_baseline(&history, &current).is_none());
	}

	#[test]
	fn pick_baseline_skips_resets() {
		let now = Timestamp::from_second(10_000).unwrap();
		let current = Snapshot {
			taken_at: now,
			counts: [("200".to_string(), 5)].into(),
		};
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
		assert_eq!(history[0].counts.get("200").copied(), Some(2));
	}

	#[test]
	fn humanise_window_formats_seconds_and_minutes() {
		assert_eq!(humanise_window(Duration::from_secs(10)), "10s");
		assert_eq!(humanise_window(Duration::from_secs(60)), "1m");
		assert_eq!(humanise_window(Duration::from_secs(540)), "9m");
		assert_eq!(humanise_window(Duration::from_secs(545)), "9m 5s");
	}
}
