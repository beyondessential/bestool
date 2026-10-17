//! Sync sessions central has left open that it should have closed.
//!
//! Central closes a session once its device has been silent for the stale
//! session window, and closes a device's earlier open session when the device
//! starts a new one. A device that drops off in the field therefore leaves a
//! session open for a while and then has it closed, which is not a fault in
//! central. The check grades an open session by how long its device has been
//! silent, not by how long ago it started: one past the window, plus a grace for
//! central's own sweep, is one central failed to close.
//!
//! spec: CHK-SSN

use jiff::Timestamp;

use super::{TamanuCx, query_error_check, row_error_check};
use crate::Stat;
use crate::check::Check;

const NAME: &str = "sync_sessions";

/// Seconds beyond central's stale session window at which a session left open
/// fails rather than warns. Central sweeps for stale sessions every minute, so
/// this is the room that sweep is given to catch up.
const FAIL_GRACE_SECONDS: u64 = 15 * 60;

/// Central's stale session window for this deployment, in seconds.
pub(super) fn stale_window_seconds(ctx: &TamanuCx) -> u64 {
	ctx.installed_config().map_or(
		bestool_tamanu::config::TamanuConfig::DEFAULT_STALE_SYNC_SESSION_SECONDS,
		|config| config.stale_sync_session_seconds(),
	)
}

// The completed_at predicate lives in the inner WHERE rather than on each
// aggregate FILTER: Postgres can't push FILTER predicates into an index, so
// the filtered form seq-scans the whole table (millions of rows on
// long-lived centrals) while this form hits the completed_at index.
//
// A session whose device never connected after starting is as silent as its
// start.
const SQL: &str = "SELECT \
		count(*) AS active_count, \
		count(*) FILTER (WHERE silent_seconds > $1::float8) AS left_open, \
		count(*) FILTER (WHERE silent_seconds > $1::float8 + $2::float8) AS past_grace \
	FROM ( \
		SELECT EXTRACT(EPOCH FROM now() - COALESCE(last_connection_time, start_time))::float8 \
			AS silent_seconds \
		FROM sync_sessions \
		WHERE completed_at IS NULL \
	) open_sessions";

/// The open sessions, and how many of them central has left open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Open {
	active: i64,
	/// Sessions whose device has been silent past the stale session window.
	left_open: i64,
	/// Sessions silent past the window and the grace beyond it.
	past_grace: i64,
}

impl Open {
	fn from_row(row: &tokio_postgres::Row) -> Result<Self, tokio_postgres::Error> {
		Ok(Self {
			active: row.try_get("active_count")?,
			left_open: row.try_get("left_open")?,
			past_grace: row.try_get("past_grace")?,
		})
	}
}

fn report(open: Open, window_seconds: u64) -> Check {
	let summary = format!("{} open, {} left open", open.active, open.left_open);
	let reason = |n: i64| {
		format!(
			"{n} session(s) left open past central's {}m stale session window",
			window_seconds / 60
		)
	};

	let check = if open.past_grace > 0 {
		Check::fail(NAME, summary, reason(open.past_grace))
	} else if open.left_open > 0 {
		Check::warning(NAME, summary, reason(open.left_open))
	} else {
		Check::pass(NAME, summary)
	};

	check
		.with_detail("active_count", open.active)
		.with_detail("left_open_count", open.left_open)
		.with_detail("stale_window_seconds", window_seconds)
		.with_stat(Stat::gauge("active", open.active as f64).help("Active sync sessions"))
}

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let window = stale_window_seconds(&ctx);
	let params: [&(dyn tokio_postgres::types::ToSql + Sync); 2] =
		[&(window as f64), &(FAIL_GRACE_SECONDS as f64)];
	let row = match client.query_one(SQL, &params).await {
		Ok(row) => row,
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& db.code() == &tokio_postgres::error::SqlState::UNDEFINED_TABLE
			{
				return Check::skip(NAME, "sync_sessions table not present", "table absent");
			}
			return query_error_check(NAME, &err);
		}
	};
	let open = match Open::from_row(&row) {
		Ok(open) => open,
		Err(err) => return row_error_check(NAME, &err),
	};

	let mut check = report(open, window);

	// Phase durations of the most recently completed session, so operators can
	// see how long each phase of a sync takes over time. Grading is unchanged;
	// these are additional telemetry only.
	let durations = last_session_phase_durations(&client).await;
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

#[cfg(test)]
mod tests {
	use super::{Open, report, stale_window_seconds};
	use crate::check::CheckStatus;
	use crate::checks::test_support::{central_ctx, facility_ctx};

	fn open(active: i64, left_open: i64, past_grace: i64) -> Open {
		Open {
			active,
			left_open,
			past_grace,
		}
	}

	#[test]
	fn open_sessions_still_within_the_window_pass_however_many_there_are() {
		let check = report(open(40, 0, 0), 3600);
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.summary, "40 open, 0 left open");
	}

	#[test]
	fn a_session_just_past_the_window_warns() {
		let check = report(open(5, 1, 0), 3600);
		assert!(matches!(check.status, CheckStatus::Warning(_)));
	}

	#[test]
	fn a_session_past_the_window_and_the_grace_fails() {
		let check = report(open(5, 2, 1), 3600);
		let CheckStatus::Fail(reason) = &check.status else {
			panic!("expected fail, got {:?}", check.status);
		};
		assert!(reason.contains("1 session(s)"));
		assert!(reason.contains("60m"));
	}

	#[test]
	fn the_detail_carries_the_counts_and_the_window() {
		let check = report(open(5, 2, 1), 1800);
		assert_eq!(check.details["active_count"], 5);
		assert_eq!(check.details["left_open_count"], 2);
		assert_eq!(check.details["stale_window_seconds"], 1800);
	}

	#[test]
	fn the_window_defaults_to_an_hour_without_an_installed_config() {
		let mut ctx = facility_ctx();
		ctx.install_root = None;
		assert_eq!(stale_window_seconds(&ctx), 3600);
	}

	// `runs_against_central` accepts a failed query as a result, so this is what
	// says the SQL is valid against the schema.
	#[tokio::test]
	async fn the_query_prepares_against_the_schema() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");
		client.prepare(super::SQL).await.expect("prepare");
	}

	// A temp table shadows the real one for this connection only, so the query
	// runs over seeded sessions without touching the database's own.
	#[tokio::test]
	async fn grades_open_sessions_by_device_silence_not_age() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");
		client
			.batch_execute(
				"CREATE TEMP TABLE sync_sessions (start_time timestamptz, \
					last_connection_time timestamptz, completed_at timestamptz); \
				INSERT INTO sync_sessions VALUES \
				(now() - '8h'::interval, now() - '1 minute'::interval, NULL), \
				(now() - '80 minutes'::interval, now() - '30 minutes'::interval, NULL), \
				(now() - '80 minutes'::interval, now() - '70 minutes'::interval, NULL), \
				(now() - '5h'::interval, now() - '3h'::interval, NULL), \
				(now() - '5h'::interval, NULL, NULL), \
				(now() - '5h'::interval, now() - '3h'::interval, now() - '2h'::interval)",
			)
			.await
			.expect("seed");

		let row = client
			.query_one(
				super::SQL,
				&[&3600.0_f64, &(super::FAIL_GRACE_SECONDS as f64)],
			)
			.await
			.expect("query");
		let open = Open::from_row(&row).unwrap();

		assert_eq!(
			open,
			Open {
				active: 5,
				left_open: 3,
				past_grace: 2
			}
		);
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "sync_sessions");
		assert!(matches!(
			check.status,
			CheckStatus::Pass | CheckStatus::Warning(_) | CheckStatus::Fail(_)
		));
	}
}
