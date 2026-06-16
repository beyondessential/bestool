//! Leftover sync-snapshot tables.
//!
//! Central's sync builds a per-session set of tables in the `sync_snapshots`
//! schema and drops them when the session finishes. A buildup means sessions
//! are dying without cleaning up after themselves. There's no fixed "too many"
//! — it scales with how much syncing the server does — so we compare the table
//! count against the number of sync sessions in the last 24h: more tables than
//! recent sessions (plus a 10% margin) warns; more than double fails.

use bestool_tamanu::ApiServerKind;

use super::{CheckContext, query_error_check};
use crate::doctor::check::Check;

const NAME: &str = "sync_snapshot_tables";

pub async fn run(ctx: CheckContext) -> Check {
	if ctx.kind != ApiServerKind::Central {
		return Check::skip(
			NAME,
			"not applicable on facility server",
			"central-only check",
		);
	}
	let Some(client) = ctx.db.as_ref() else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	// `pg_tables` for a missing schema simply yields 0 rows, so only the
	// `sync_sessions` lookup can hit an undefined table.
	let query = "
		SELECT
			(SELECT count(*) FROM pg_tables WHERE schemaname = 'sync_snapshots') AS table_count,
			(SELECT count(*) FROM sync_sessions WHERE start_time > now() - interval '24 hours') AS sessions_24h
	";

	let row = match client.query_one(query, &[]).await {
		Ok(r) => r,
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& db.code() == &tokio_postgres::error::SqlState::UNDEFINED_TABLE
			{
				return Check::skip(NAME, "sync_sessions table not present", "table absent");
			}
			return query_error_check(NAME, &err);
		}
	};

	let tables: i64 = row.try_get("table_count").unwrap_or(0);
	let sessions: i64 = row.try_get("sessions_24h").unwrap_or(0);

	let summary = format!("{tables} snapshot table(s), {sessions} sync session(s)/24h");
	let check = match classify(tables, sessions) {
		Verdict::Pass => Check::pass(NAME, summary),
		Verdict::Warn(reason) => Check::warning(NAME, summary, reason),
		Verdict::Fail(reason) => Check::fail(NAME, summary, reason),
	};
	check
		.with_detail("table_count", tables)
		.with_detail("sessions_24h", sessions)
}

enum Verdict {
	Pass,
	Warn(String),
	Fail(String),
}

/// Compare the leftover snapshot-table count against recent sync activity.
///
/// Warn once the table count exceeds the last-24h session count plus a 10%
/// margin; fail once it exceeds double that session count. With no recent
/// sessions any leftover tables are a leak, so both thresholds collapse to
/// zero and a non-empty schema fails.
fn classify(tables: i64, sessions_24h: i64) -> Verdict {
	let warn_at = sessions_24h as f64 * 1.1;
	let fail_at = sessions_24h as f64 * 2.0;
	let t = tables as f64;
	if t > fail_at {
		Verdict::Fail(format!(
			"{tables} snapshot tables is more than double the {sessions_24h} sync session(s) in the last 24h"
		))
	} else if t > warn_at {
		Verdict::Warn(format!(
			"{tables} snapshot tables exceeds the {sessions_24h} sync session(s) in the last 24h plus 10%"
		))
	} else {
		Verdict::Pass
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn verdict(tables: i64, sessions: i64) -> &'static str {
		match classify(tables, sessions) {
			Verdict::Pass => "pass",
			Verdict::Warn(_) => "warn",
			Verdict::Fail(_) => "fail",
		}
	}

	#[test]
	fn within_recent_activity_passes() {
		assert_eq!(verdict(100, 100), "pass");
		// Exactly +10% is the boundary and still passes.
		assert_eq!(verdict(110, 100), "pass");
	}

	#[test]
	fn modest_excess_warns() {
		assert_eq!(verdict(120, 100), "warn");
		assert_eq!(verdict(200, 100), "warn");
	}

	#[test]
	fn more_than_double_fails() {
		assert_eq!(verdict(201, 100), "fail");
	}

	#[test]
	fn leftover_with_no_recent_sessions_fails() {
		// No syncs in 24h, but snapshot tables are present — a leak.
		assert_eq!(verdict(5, 0), "fail");
	}

	#[test]
	fn empty_schema_always_passes() {
		assert_eq!(verdict(0, 0), "pass");
		assert_eq!(verdict(0, 50), "pass");
	}
}
