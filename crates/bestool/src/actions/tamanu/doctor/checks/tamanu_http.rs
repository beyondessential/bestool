use std::time::{Duration, Instant};

use super::CheckContext;
use crate::actions::tamanu::doctor::check::Check;

const PING_URL: &str = "http://localhost/api/public/ping";
const TIMEOUT: Duration = Duration::from_secs(5);

pub async fn run(_ctx: CheckContext) -> Check {
	let client = match reqwest::Client::builder().timeout(TIMEOUT).build() {
		Ok(c) => c,
		Err(err) => {
			return Check::fail("tamanu_http", "client build failed", err.to_string())
				.with_detail("url", PING_URL);
		}
	};

	let start = Instant::now();
	let response = client.get(PING_URL).send().await;
	let latency_ms = start.elapsed().as_millis() as u64;

	let check = match response {
		Ok(resp) => {
			let status = resp.status();
			let detail_status = status.as_u16();
			if status.is_success() {
				Check::pass(
					"tamanu_http",
					format!("HTTP {} from {PING_URL} ({latency_ms}ms)", status.as_u16()),
				)
				.with_detail("status_code", detail_status)
			} else {
				Check::fail(
					"tamanu_http",
					format!("HTTP {} from {PING_URL}", status.as_u16()),
					format!("non-success status {status}"),
				)
				.with_detail("status_code", detail_status)
			}
		}
		Err(err) => Check::fail(
			"tamanu_http",
			format!("could not reach {PING_URL}"),
			err.to_string(),
		),
	};

	check.with_detail("url", PING_URL)
		.with_detail("latency_ms", latency_ms)
}
