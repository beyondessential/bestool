use std::time::Instant;

use super::{PgCx, fmt_db_error};
use crate::Stat;
use crate::check::Check;

/// Connect latency above which the DB is treated as degraded.
const WARN_LATENCY_MS: u64 = 1000;

pub async fn run(ctx: PgCx) -> Check {
	let host = ctx
		.database
		.host
		.clone()
		.unwrap_or_else(|| "localhost".into());
	let name = ctx.database.name.clone();

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
					"connect",
					summary,
					format!("connect latency {latency_ms}ms over {WARN_LATENCY_MS}ms"),
				)
			} else {
				Check::pass("connect", summary)
			}
		}
		Err(err) => Check::fail(
			"connect",
			format!("failed to connect to {host}/{name}"),
			fmt_db_error(&err),
		),
	};

	check
		.with_detail("db_host", host)
		.with_detail("db_name", name)
		.with_detail("latency_ms", latency_ms)
		.with_stat(Stat::gauge("latency_ms", latency_ms as f64).help("Postgres connect latency"))
}

#[cfg(test)]
mod tests {
	use bestool_tamanu::config::Database;

	use super::*;
	use crate::{check::CheckStatus, subject::ApplicationRef};

	/// An unreachable postgres must surface as a FAIL (an alert), never a crash
	/// or hang — this is what lets the daemon flag a down database. Port 1 has
	/// nothing listening, so the connection is refused immediately.
	#[tokio::test]
	async fn unreachable_postgres_alerts() {
		let url = "postgresql://127.0.0.1:1/tamanu-central";
		let ctx = PgCx {
			app: ApplicationRef::local_postgres(1),
			database: Database::from_url(url).unwrap(),
			database_url: url.into(),
			pool: None,
		};
		let check = run(ctx).await;
		assert!(
			matches!(check.status, CheckStatus::Fail(_)),
			"expected FAIL on unreachable postgres, got {:?}",
			check.status
		);
	}

	/// A reachable postgres that actively rejects the connection (here, a
	/// nonexistent database name) produces a real `DbError`, not an IO
	/// failure. The reported reason must be that error's actual message, not
	/// `tokio_postgres::Error`'s generic top-level Display for `Kind::Db`
	/// ("db error") — which is what `fmt_db_error` exists to avoid. Skips if
	/// there's no local test postgres, mirroring the other DB-backed tests in
	/// this crate.
	#[tokio::test]
	async fn rejected_connection_surfaces_real_reason() {
		if crate::checks::test_support::central_ctx().await.is_none() {
			return;
		}

		let url = "postgresql://localhost/bestool-test-nonexistent-db";
		let ctx = PgCx {
			app: ApplicationRef::local_postgres(5432),
			database: Database::from_url(url).unwrap(),
			database_url: url.into(),
			pool: None,
		};
		let check = run(ctx).await;
		match check.status {
			// Postgres rejects the startup packet with SQLSTATE 3D000
			// (invalid_catalog_name), FATAL severity, and a message naming the
			// missing database — e.g. `FATAL: database "..." does not exist`.
			CheckStatus::Fail(reason) => {
				assert!(
					reason.contains("does not exist"),
					"expected postgres's real 'database does not exist' rejection, got {reason:?}"
				);
				assert!(
					reason.contains("bestool-test-nonexistent-db"),
					"expected the message to name the missing database, got {reason:?}"
				);
			}
			other => panic!("expected FAIL on a rejected connection, got {other:?}"),
		}
	}
}
