//! Whether a facility server's sync has been running for too long.
//!
//! A sync attempt is recorded before it contacts central and given a completion
//! time when it ends, however it ends, so one that hangs never gets one. Only the
//! most recent attempt is graded: an attempt whose process died is also left
//! without one, but once a later attempt has started it is no longer what the
//! facility is doing.
//!
//! spec: CHK-SFH

use super::{TamanuCx, query_error_check, row_error_check};
use crate::check::Check;

const NAME: &str = "sync_facility_hung";

const FAIL_MINUTES: f64 = 60.0;

const SQL: &str = "SELECT completed_at IS NOT NULL AS finished, start_time::text AS start_time, session_id, \
		persist_started_at::text AS persist_started_at, \
		persist_completed_at::text AS persist_completed_at, \
		(EXTRACT(EPOCH FROM (now() - start_time)) / 60)::float8 AS minutes_running \
	FROM sync_facility_runs \
	WHERE deleted_at IS NULL \
	ORDER BY start_time DESC LIMIT 1";

struct Run {
	finished: bool,
	start_time: String,
	session_id: Option<String>,
	persist_started_at: Option<String>,
	persist_completed_at: Option<String>,
	minutes_running: f64,
}

impl Run {
	fn from_row(row: &tokio_postgres::Row) -> Result<Self, tokio_postgres::Error> {
		Ok(Self {
			finished: row.try_get("finished")?,
			start_time: row.try_get("start_time")?,
			session_id: row.try_get("session_id")?,
			persist_started_at: row.try_get("persist_started_at")?,
			persist_completed_at: row.try_get("persist_completed_at")?,
			minutes_running: row.try_get("minutes_running")?,
		})
	}

	/// Where the attempt has got to, so one stuck saving pulled changes can be
	/// told apart from one stuck talking to central.
	fn stage(&self) -> &'static str {
		match (&self.persist_started_at, &self.persist_completed_at) {
			(Some(_), None) => "saving pulled changes",
			_ => "exchanging changes with central",
		}
	}
}

fn report(run: Option<&Run>) -> Check {
	let Some(run) = run else {
		return Check::pass(NAME, "no sync runs");
	};
	if run.finished {
		return Check::pass(NAME, "no sync running");
	}

	let minutes = run.minutes_running;
	let check = if minutes > FAIL_MINUTES {
		Check::fail(
			NAME,
			format!("sync running for {minutes:.0}m"),
			format!("sync running for {minutes:.0} minutes, {}", run.stage()),
		)
	} else {
		Check::pass(NAME, format!("sync running for {minutes:.0}m"))
	};

	check
		.with_detail("start_time", run.start_time.clone())
		.with_detail("minutes_running", minutes)
		.with_detail("session_id", run.session_id.clone())
		.with_detail("persist_started_at", run.persist_started_at.clone())
		.with_detail("persist_completed_at", run.persist_completed_at.clone())
}

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let row = match client.query_opt(SQL, &[]).await {
		Ok(row) => row,
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& db.code() == &tokio_postgres::error::SqlState::UNDEFINED_TABLE
			{
				return Check::skip(NAME, "sync_facility_runs table not present", "table absent");
			}
			return query_error_check(NAME, &err);
		}
	};

	match row.as_ref().map(Run::from_row).transpose() {
		Ok(run) => report(run.as_ref()).with_detail("fail_minutes", FAIL_MINUTES),
		Err(err) => row_error_check(NAME, &err),
	}
}

#[cfg(test)]
mod tests {
	use super::{Run, report};
	use crate::check::CheckStatus;
	use crate::checks::test_support::{central_ctx, facility_ctx};

	fn run(finished: bool, minutes: f64) -> Run {
		Run {
			finished,
			start_time: "2026-01-01 00:00:00+00".into(),
			session_id: None,
			persist_started_at: None,
			persist_completed_at: None,
			minutes_running: minutes,
		}
	}

	#[test]
	fn a_run_going_for_over_an_hour_fails() {
		let check = report(Some(&run(false, 61.0)));
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		assert_eq!(check.details["minutes_running"], 61.0);
	}

	#[test]
	fn a_run_going_for_up_to_an_hour_passes() {
		assert!(matches!(
			report(Some(&run(false, 60.0))).status,
			CheckStatus::Pass
		));
	}

	#[test]
	fn a_latest_run_that_has_ended_passes_however_long_ago_it_started() {
		assert!(matches!(
			report(Some(&run(true, 600.0))).status,
			CheckStatus::Pass
		));
	}

	#[test]
	fn no_run_passes() {
		assert!(matches!(report(None).status, CheckStatus::Pass));
	}

	#[test]
	fn a_run_stuck_saving_says_so() {
		let stuck = Run {
			persist_started_at: Some("2026-01-01 00:05:00+00".into()),
			..run(false, 90.0)
		};
		let CheckStatus::Fail(reason) = report(Some(&stuck)).status else {
			panic!("expected fail");
		};
		assert!(reason.contains("saving pulled changes"), "{reason}");

		let CheckStatus::Fail(reason) = report(Some(&run(false, 90.0))).status else {
			panic!("expected fail");
		};
		assert!(reason.contains("exchanging changes"), "{reason}");
	}

	#[tokio::test]
	async fn skips_without_a_db() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}

	// The table ships on central too, so central's schema is enough to say the
	// query is valid once Tamanu carries it.
	#[tokio::test]
	async fn the_query_prepares_against_the_schema() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");
		if let Err(err) = client.prepare(super::SQL).await {
			let undefined = err
				.as_db_error()
				.is_some_and(|db| db.code() == &tokio_postgres::error::SqlState::UNDEFINED_TABLE);
			assert!(undefined, "prepare: {err}");
		}
	}
}
