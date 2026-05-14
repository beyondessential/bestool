//! HTTP error rate, sourced from Caddy's Prometheus metrics endpoint.
//!
//! Caddy's admin API at `localhost:2019` exposes `/metrics` in Prometheus text
//! format. The relevant series is `caddy_http_requests_total{code="…",…}`,
//! a cumulative counter labelled with the HTTP status code. We aggregate by
//! status class and report the cumulative error rate.

use std::time::Duration;

use serde_json::{Map, Value};

use super::CheckContext;
use crate::actions::tamanu::doctor::check::Check;

const CADDY_METRICS_URL: &str = "http://localhost:2019/metrics";
const TIMEOUT: Duration = Duration::from_secs(3);

const WARN_ERROR_PCT: f64 = 5.0;
const FAIL_ERROR_PCT: f64 = 20.0;

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

	let body = match client.get(CADDY_METRICS_URL).send().await {
		Ok(resp) if resp.status().is_success() => match resp.text().await {
			Ok(t) => t,
			Err(err) => {
				return Check::pass(
					"http_errors",
					"caddy /metrics body read failed",
				)
				.with_detail("skipped", true)
				.with_detail("reason", err.to_string());
			}
		},
		Ok(resp) => {
			return Check::pass(
				"http_errors",
				format!("caddy /metrics returned HTTP {}", resp.status().as_u16()),
			)
			.with_detail("skipped", true);
		}
		Err(_) => {
			return Check::pass("http_errors", "caddy admin unreachable")
				.with_detail("skipped", true);
		}
	};

	let counts = parse_status_counts(&body);
	let total: u64 = counts.iter().map(|(_, n)| n).sum();
	let errored: u64 = counts
		.iter()
		.filter(|(code, _)| {
			let first = code.chars().next();
			matches!(first, Some('4') | Some('5'))
		})
		.map(|(_, n)| n)
		.sum();

	if total == 0 {
		return Check::pass("http_errors", "no requests observed yet")
			.with_detail("total", 0u64);
	}

	let pct = (errored as f64 / total as f64) * 100.0;
	let summary = format!("{errored}/{total} errored ({pct:.1}% cumulative)");

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
	for (code, n) in &counts {
		by_code.insert(code.clone(), Value::from(*n));
	}

	check.with_detail("total_requests", total)
		.with_detail("error_requests", errored)
		.with_detail("error_rate_pct", pct)
		.with_detail("by_code", Value::Object(by_code))
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
fn parse_status_counts(body: &str) -> Vec<(String, u64)> {
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

	let mut totals: HashMap<String, u64> = HashMap::new();
	for ((_, _, _, code), count) in per_tuple {
		*totals.entry(code).or_insert(0) += count;
	}

	let mut entries: Vec<(String, u64)> = totals.into_iter().collect();
	entries.sort_by(|a, b| a.0.cmp(&b.0));
	entries
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

	#[test]
	fn parses_caddy_metric_lines() {
		let counts = parse_status_counts(SAMPLE);
		// 200 entries dedupe across handlers (max=9 for the (host,method,server,code)
		// tuple); 404 and 502 each have a single handler entry.
		assert_eq!(
			counts,
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
}
