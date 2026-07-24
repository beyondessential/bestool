use jiff::Timestamp;

use super::{CheckContext, query_error_check};
use crate::doctor::Stat;
use crate::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::fail("sync_sessions", "no DB connection", "db_connect failed");
	};

	// The completed_at predicate lives in the outer WHERE rather than on each
	// aggregate FILTER: Postgres can't push FILTER predicates into an index, so
	// the filtered form seq-scans the whole table (millions of rows on
	// long-lived centrals) while this form hits the completed_at index.
	let query = r#"
		SELECT
			count(*) AS active_count,
			count(*) FILTER (
				WHERE start_time < now() - interval '15 minutes'
			) AS stuck_warn,
			count(*) FILTER (
				WHERE start_time < now() - interval '45 minutes'
			) AS stuck_fail,
			min(start_time) AS oldest_started_at
		FROM sync_sessions
		WHERE completed_at IS NULL
	"#;

	let row = match client.query_opt(query, &[]).await {
		Ok(Some(r)) => r,
		Ok(None) => {
			return Check::pass("sync_sessions", "no sync sessions")
				.with_detail("active_count", 0)
				.with_stat(Stat::gauge("active", 0.0).help("Active sync sessions"))
				.with_stat(Stat::gauge("stuck_15m", 0.0))
				.with_stat(Stat::gauge("stuck_45m", 0.0));
		}
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& db.code() == &tokio_postgres::error::SqlState::UNDEFINED_TABLE
			{
				return Check::skip(
					"sync_sessions",
					"sync_sessions table not present",
					"table absent",
				);
			}
			return query_error_check("sync_sessions", &err);
		}
	};

	let active: i64 = row.try_get("active_count").unwrap_or(0);
	let stuck_warn: i64 = row.try_get("stuck_warn").unwrap_or(0);
	let stuck_fail: i64 = row.try_get("stuck_fail").unwrap_or(0);
	let oldest: Option<Timestamp> = row.try_get("oldest_started_at").ok();

	let summary = format!("{active} active, {stuck_warn} stuck >15m");
	let check = if stuck_fail > 0 {
		Check::fail(
			"sync_sessions",
			summary.clone(),
			format!("{stuck_fail} session(s) stuck >45m"),
		)
	} else if stuck_warn > 0 {
		Check::warning(
			"sync_sessions",
			summary.clone(),
			format!("{stuck_warn} session(s) stuck >15m"),
		)
	} else {
		Check::pass("sync_sessions", summary)
	};

	let mut check = check
		.with_detail("active_count", active)
		.with_detail("stuck_count", stuck_warn)
		.with_stat(Stat::gauge("active", active as f64).help("Active sync sessions"))
		.with_stat(
			Stat::gauge("stuck_15m", stuck_warn as f64).help("Sessions running over 15 minutes"),
		)
		.with_stat(
			Stat::gauge("stuck_45m", stuck_fail as f64).help("Sessions running over 45 minutes"),
		);
	if let Some(ts) = oldest {
		check = check.with_detail("oldest_started_at", ts.to_string());
	}
	check
}
