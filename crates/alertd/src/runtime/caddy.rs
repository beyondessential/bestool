//! Reading the traffic a Tamanu deployment serves, through the Caddy in front
//! of it on this machine.
//!
//! Caddy's admin API at `localhost:2019` exposes `/metrics` in Prometheus text
//! format, and the relevant series is the request-duration histogram's count,
//! labelled with the HTTP status code.
//!
//! One local Caddy fronts the whole machine, so it is one source. Where traffic
//! is served by shared infrastructure instead, a substrate reads that
//! infrastructure's counts and filters them to the application being reported
//! for — the reason the counters come back per source rather than as one total.
//!
//! Caddy instruments its handlers only when its config switches metrics on, and
//! serves `/metrics` with its process and admin series either way. So finding no
//! request counters at all says nothing about how busy the server is, and this
//! reads the config to tell an idle server from an uninstrumented one: an
//! uninstrumented Caddy is a reading that cannot be taken, not a quiet one.
//!
//! spec: SUB#http-traffic-and-certificates

use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use super::{HttpRuntime, TrafficCounters, TrafficSource, Unavailable};
use crate::checks::fmt_chain;

const METRICS_URL: &str = "http://localhost:2019/metrics";
const CONFIG_URL: &str = "http://localhost:2019/config/apps/http";
const TIMEOUT: Duration = Duration::from_secs(3);

/// What the one Caddy on this machine is named as a source of counts.
///
/// A machine has one, and it fronts everything on it, so the name is fixed. It
/// exists at all because a substrate whose counts come from several front ends
/// keeps history per source, and a source that vanishes must be dropped rather
/// than read as the quantity having fallen.
const SOURCE: &str = "caddy";

/// The Caddy serving a deployment on the machine this process is on.
pub struct CaddyRuntime {
	/// Shared with the checks so TCP connections stay warm between ticks.
	http: reqwest::Client,
}

impl CaddyRuntime {
	pub fn new(http: reqwest::Client) -> Self {
		Self { http }
	}

	/// Caddy's own view of whether it counts requests.
	///
	/// Only asked when no counters came back at all: that is either a quiet
	/// server or one not counting, and the two are a reading and the absence of
	/// one.
	async fn instrumented(&self) -> Result<bool, Unavailable> {
		let config = match self.http.get(CONFIG_URL).timeout(TIMEOUT).send().await {
			Ok(resp) if resp.status().is_success() => resp.json::<Value>().await,
			Ok(resp) => {
				debug!(status = %resp.status(), "caddy config endpoint refused");
				return Err(Unavailable::new(format!(
					"caddy reported no requests at all, and its config at {CONFIG_URL} answered HTTP {} — whether it counts requests could not be established",
					resp.status().as_u16()
				)));
			}
			Err(err) => {
				return Err(Unavailable::new(format!(
					"caddy reported no requests at all, and its config at {CONFIG_URL} could not be read ({}) — whether it counts requests could not be established",
					fmt_chain(&err)
				)));
			}
		};

		match config {
			Ok(config) => Ok(metrics_enabled_in(&config)),
			Err(err) => Err(Unavailable::new(format!(
				"caddy reported no requests at all, and its config at {CONFIG_URL} did not parse ({}) — whether it counts requests could not be established",
				fmt_chain(&err)
			))),
		}
	}
}

#[async_trait]
impl HttpRuntime for CaddyRuntime {
	async fn http_counters(&self) -> Result<TrafficCounters, Unavailable> {
		let body = match self.http.get(METRICS_URL).timeout(TIMEOUT).send().await {
			Ok(resp) if resp.status().is_success() => resp.text().await.map_err(|err| {
				Unavailable::new(format!(
					"caddy /metrics body read failed: {}",
					fmt_chain(&err)
				))
			})?,
			Ok(resp) => {
				let status = resp.status().as_u16();
				return Err(Unavailable::new(format!(
					"caddy is reachable but its admin /metrics endpoint isn't usable (HTTP {status}) — error rate cannot be measured"
				)));
			}
			Err(err) => {
				return Err(Unavailable::new(format!(
					"could not reach caddy admin at {METRICS_URL}: {}",
					fmt_chain(&err)
				)));
			}
		};

		let by_status = parse_status_counts(&body);
		if by_status.is_empty() && !self.instrumented().await? {
			return Err(Unavailable::new(
				"caddy counts requests only when its config asks it to, so there is no error rate to grade. Add `metrics` to the Caddyfile's global options — within a `servers` block on caddy older than 2.9.",
			));
		}

		Ok(TrafficCounters {
			sources: vec![TrafficSource {
				source: SOURCE.to_string(),
				by_status,
			}],
		})
	}
}

/// Whether caddy's http app config switches request metrics on. Caddy 2.9 moved
/// the switch onto the app itself; before that each server carried its own, and
/// one instrumented server is enough to produce counters.
fn metrics_enabled_in(http_app: &Value) -> bool {
	if !http_app["metrics"].is_null() {
		return true;
	}
	http_app["servers"]
		.as_object()
		.is_some_and(|servers| servers.values().any(|server| !server["metrics"].is_null()))
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
	fn metrics_switch_read_from_the_app_or_its_servers() {
		use serde_json::json;

		// caddy 2.9 and later: the switch sits on the http app, and caddy
		// serialises it as an empty object when it carries no sub-options.
		assert!(metrics_enabled_in(&json!({ "metrics": {} })));
		assert!(metrics_enabled_in(
			&json!({ "metrics": { "per_host": true } })
		));

		// before 2.9: per server, and one instrumented server produces counters.
		assert!(metrics_enabled_in(&json!({
			"servers": { "srv0": { "metrics": {} }, "srv1": { "listen": [":80"] } }
		})));

		assert!(!metrics_enabled_in(&json!({
			"servers": { "srv0": { "listen": [":443"], "routes": [] } }
		})));
		assert!(!metrics_enabled_in(&json!({ "servers": {} })));
		assert!(!metrics_enabled_in(&json!({})));
		// caddy answers with a bare null when it has no http app at all
		assert!(!metrics_enabled_in(&Value::Null));
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
