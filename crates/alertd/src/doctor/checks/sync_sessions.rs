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
				.with_stat(Stat::gauge("active", 0.0).help("Active sync sessions"));
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
		.with_stat(Stat::gauge("active", active as f64).help("Active sync sessions"));
	if let Some(ts) = oldest {
		check = check.with_detail("oldest_started_at", ts.to_string());
	}

	// Phase durations of the most recently completed session, so operators can
	// see how long each phase of a sync takes over time. Grading is unchanged;
	// these are additional telemetry only.
	let durations = last_session_phase_durations(client).await;
	if let Some(s) = durations.snapshot {
		check = check.with_stat(
			Stat::gauge("snapshot_duration_seconds", s)
				.group("durations")
				.help("Snapshot phase of the last completed sync"),
		);
	}
	if let Some(s) = durations.persist {
		check = check.with_stat(
			Stat::gauge("persist_duration_seconds", s)
				.group("durations")
				.help("Persist phase of the last completed sync"),
		);
	}
	if let Some(s) = durations.total {
		check = check.with_stat(
			Stat::gauge("total_duration_seconds", s)
				.group("durations")
				.help("Total duration of the last completed sync"),
		);
	}
	check
}

/// Phase durations, in seconds, of one sync session.
struct PhaseDurations {
	snapshot: Option<f64>,
	persist: Option<f64>,
	total: Option<f64>,
}

/// Durations of the phases of the most recently completed sync session:
/// the snapshot phase, the persist phase, and the session overall. A phase
/// whose timestamps are missing (an older session, or one that skipped it) is
/// left out. Returns all-absent when no session has completed or the query
/// fails — this is telemetry, never a reason to fail the check.
async fn last_session_phase_durations(client: &tokio_postgres::Client) -> PhaseDurations {
	let none = PhaseDurations {
		snapshot: None,
		persist: None,
		total: None,
	};
	let query = "SELECT snapshot_started_at, snapshot_completed_at, persist_completed_at, \
		start_time, completed_at \
		FROM sync_sessions WHERE completed_at IS NOT NULL \
		ORDER BY completed_at DESC LIMIT 1";
	let Ok(Some(row)) = client.query_opt(query, &[]).await else {
		return none;
	};
	let at = |col| row.try_get::<_, Option<Timestamp>>(col).ok().flatten();
	let between = |end: Option<Timestamp>, start: Option<Timestamp>| match (end, start) {
		(Some(end), Some(start)) => Some((end - start).get_seconds().max(0) as f64),
		_ => None,
	};
	let snapshot_started = at("snapshot_started_at");
	let snapshot_completed = at("snapshot_completed_at");
	PhaseDurations {
		snapshot: between(snapshot_completed, snapshot_started),
		persist: between(at("persist_completed_at"), snapshot_completed),
		total: between(at("completed_at"), at("start_time")),
	}
}
