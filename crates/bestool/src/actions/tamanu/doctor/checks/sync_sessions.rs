use jiff::Timestamp;

use super::CheckContext;
use crate::actions::tamanu::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::fail("sync_sessions", "no DB connection", "db_connect failed");
	};

	let query = r#"
		SELECT
			count(*) FILTER (WHERE "completedAt" IS NULL) AS active_count,
			count(*) FILTER (
				WHERE "completedAt" IS NULL AND "startTime" < now() - interval '1 hour'
			) AS stuck_warn,
			count(*) FILTER (
				WHERE "completedAt" IS NULL AND "startTime" < now() - interval '6 hours'
			) AS stuck_fail,
			min("startTime") FILTER (WHERE "completedAt" IS NULL) AS oldest_started_at
		FROM sync_sessions
	"#;

	let row = match client.query_opt(query, &[]).await {
		Ok(Some(r)) => r,
		Ok(None) => {
			return Check::pass("sync_sessions", "no sync sessions")
				.with_detail("active_count", 0);
		}
		Err(err) => {
			let msg = err.to_string();
			if msg.contains("sync_sessions")
				&& (msg.contains("does not exist") || msg.contains("no such"))
			{
				return Check::pass(
					"sync_sessions",
					"sync_sessions table not present",
				)
				.with_detail("skipped", true);
			}
			return Check::fail("sync_sessions", "query failed", msg);
		}
	};

	let active: i64 = row.try_get("active_count").unwrap_or(0);
	let stuck_warn: i64 = row.try_get("stuck_warn").unwrap_or(0);
	let stuck_fail: i64 = row.try_get("stuck_fail").unwrap_or(0);
	let oldest: Option<Timestamp> = row.try_get("oldest_started_at").ok();

	let summary = format!("{active} active, {stuck_warn} stuck >1h");
	let check = if stuck_fail > 0 {
		Check::fail(
			"sync_sessions",
			summary.clone(),
			format!("{stuck_fail} session(s) stuck >6h"),
		)
	} else if stuck_warn > 0 {
		Check::warning(
			"sync_sessions",
			summary.clone(),
			format!("{stuck_warn} session(s) stuck >1h"),
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
