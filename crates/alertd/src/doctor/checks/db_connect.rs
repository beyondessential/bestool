use std::time::Instant;

use super::CheckContext;
use crate::doctor::check::Check;

/// Connect latency above which the DB is treated as degraded.
const WARN_LATENCY_MS: u64 = 1000;

pub async fn run(ctx: CheckContext) -> Check {
	let host = ctx
		.config
		.db
		.host
		.clone()
		.unwrap_or_else(|| "localhost".into());
	let name = ctx.config.db.name.clone();

	let start = Instant::now();
	let connect_result = tokio_postgres::connect(&ctx.database_url, tokio_postgres::NoTls).await;
	let latency_ms = start.elapsed().as_millis() as u64;

	let check = match connect_result {
		Ok((_, conn)) => {
			tokio::spawn(async move {
				let _ = conn.await;
			});
			let summary = format!("postgres at {host}/{name} ({latency_ms}ms)");
			if latency_ms > WARN_LATENCY_MS {
				Check::warning(
					"db_connect",
					summary,
					format!("connect latency {latency_ms}ms over {WARN_LATENCY_MS}ms"),
				)
			} else {
				Check::pass("db_connect", summary)
			}
		}
		Err(err) => Check::fail(
			"db_connect",
			format!("failed to connect to {host}/{name}"),
			err.to_string(),
		),
	};

	check
		.with_detail("db_host", host)
		.with_detail("db_name", name)
		.with_detail("latency_ms", latency_ms)
}
