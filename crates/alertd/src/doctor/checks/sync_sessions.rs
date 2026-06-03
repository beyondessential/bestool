use jiff::Timestamp;

use super::{CheckContext, query_error_check};
use crate::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::fail("sync_sessions", "no DB connection", "db_connect failed");
	};

	let query = r#"
		SELECT
			count(*) FILTER (WHERE completed_at IS NULL) AS active_count,
			count(*) FILTER (
				WHERE completed_at IS NULL AND start_time < now() - interval '15 minutes'
			) AS stuck_warn,
			count(*) FILTER (
				WHERE completed_at IS NULL AND start_time < now() - interval '45 minutes'
			) AS stuck_fail,
			min(start_time) FILTER (WHERE completed_at IS NULL) AS oldest_started_at
		FROM sync_sessions
	"#;

	let row = match client.query_opt(query, &[]).await {
		Ok(Some(r)) => r,
		Ok(None) => {
			return Check::pass("sync_sessions", "no sync sessions").with_detail("active_count", 0);
		}
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& db.code() == &tokio_postgres::error::SqlState::UNDEFINED_TABLE
			{
				return Check::pass("sync_sessions", "sync_sessions table not present")
					.with_detail("skipped", true);
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
		.with_detail("stuck_count", stuck_warn);
	if let Some(ts) = oldest {
		check = check.with_detail("oldest_started_at", ts.to_string());
	}
	check
}
