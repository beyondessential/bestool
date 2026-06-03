use std::time::{Duration, Instant};

use super::{CheckContext, fmt_chain};
use crate::doctor::check::Check;

const PING_URL: &str = "http://localhost/api/public/ping";
const TIMEOUT: Duration = Duration::from_secs(5);
/// Response latency above which a reachable endpoint is treated as degraded.
const WARN_LATENCY_MS: u64 = 2000;

pub async fn run(ctx: CheckContext) -> Check {
	let start = Instant::now();
	let response = ctx.http_client.get(PING_URL).timeout(TIMEOUT).send().await;
	let latency_ms = start.elapsed().as_millis() as u64;

	let check = match response {
		Ok(resp) => {
			let status = resp.status();
			let detail_status = status.as_u16();
			if status.is_success() {
				let summary = format!("HTTP {} from {PING_URL} ({latency_ms}ms)", status.as_u16());
				if latency_ms > WARN_LATENCY_MS {
					Check::warning(
						"tamanu_http",
						summary,
						format!("response latency {latency_ms}ms over {WARN_LATENCY_MS}ms"),
					)
				} else {
					Check::pass("tamanu_http", summary)
				}
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
			fmt_chain(&err),
		),
	};

	check
		.with_detail("url", PING_URL)
		.with_detail("latency_ms", latency_ms)
}
