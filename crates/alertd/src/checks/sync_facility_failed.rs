//! Whether a facility server's most recent sync failed.
//!
//! Reads the facility's own record of its sync attempts. Only attempts that
//! ended as succeeded or failed are considered: a queued attempt is central
//! deferring the facility to a later turn, and one still running has no outcome
//! yet (a hung one is `sync_facility_hung`'s concern).
//!
//! spec: CHK-SFF

use super::{TamanuCx, query_error_check, row_error_check};
use crate::check::Check;

const NAME: &str = "sync_facility_failed";

const SQL: &str = "SELECT status, start_time::text AS start_time, \
		completed_at::text AS completed_at, error \
	FROM sync_facility_runs \
	WHERE status IN ('succeeded', 'failed') AND deleted_at IS NULL \
	ORDER BY start_time DESC LIMIT 1";

struct Run {
	status: String,
	start_time: String,
	completed_at: Option<String>,
	error: Option<String>,
}

impl Run {
	fn from_row(row: &tokio_postgres::Row) -> Result<Self, tokio_postgres::Error> {
		Ok(Self {
			status: row.try_get("status")?,
			start_time: row.try_get("start_time")?,
			completed_at: row.try_get("completed_at")?,
			error: row.try_get("error")?,
		})
	}
}

fn report(run: Option<&Run>) -> Check {
	let Some(run) = run else {
		return Check::pass(NAME, "no finished sync runs");
	};

	let check = if run.status == "failed" {
		let reason = match run.error.as_deref() {
			Some(error) if !error.is_empty() => format!("latest sync failed: {error}"),
			_ => "latest sync failed".to_string(),
		};
		Check::fail(NAME, "latest sync failed", reason)
	} else {
		Check::pass(NAME, "latest sync succeeded")
	};

	check
		.with_detail("start_time", run.start_time.clone())
		.with_detail("completed_at", run.completed_at.clone())
		.with_detail("error", run.error.clone())
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
		Ok(run) => report(run.as_ref()),
		Err(err) => row_error_check(NAME, &err),
	}
}

#[cfg(test)]
mod tests {
	use super::{Run, report};
	use crate::check::CheckStatus;
	use crate::checks::test_support::{central_ctx, facility_ctx};

	fn run(status: &str, error: Option<&str>) -> Run {
		Run {
			status: status.into(),
			start_time: "2026-01-01 00:00:00+00".into(),
			completed_at: Some("2026-01-01 00:01:00+00".into()),
			error: error.map(Into::into),
		}
	}

	#[test]
	fn a_failed_latest_run_fails_with_its_error() {
		let check = report(Some(&run("failed", Some("connection reset"))));
		let CheckStatus::Fail(reason) = &check.status else {
			panic!("expected fail, got {:?}", check.status);
		};
		assert!(reason.contains("connection reset"));
		assert_eq!(check.details["error"], "connection reset");
		assert_eq!(check.details["start_time"], "2026-01-01 00:00:00+00");
	}

	#[test]
	fn a_failed_run_with_no_error_message_still_fails() {
		let check = report(Some(&run("failed", None)));
		assert!(matches!(check.status, CheckStatus::Fail(_)));
	}

	#[test]
	fn a_succeeded_latest_run_passes() {
		let check = report(Some(&run("succeeded", None)));
		assert!(matches!(check.status, CheckStatus::Pass));
	}

	#[test]
	fn no_finished_run_passes() {
		assert!(matches!(report(None).status, CheckStatus::Pass));
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
