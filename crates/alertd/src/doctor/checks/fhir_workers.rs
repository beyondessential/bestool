//! FHIR materialisation worker liveness.
//!
//! Tamanu's FHIR workers register a row in `fhir.job_workers` and update its
//! `updated_at` on a periodic heartbeat; job grabbing, completion, and
//! stuck-job reclamation all gate on that heartbeat being within
//! `fhir.worker.assumeDroppedAfter` (default 10 minutes). A worker that stops
//! heartbeating stalls materialisation even while its service process looks up,
//! which neither `tamanu_service` (process up) nor `fhir_jobs` (backlog) catches.
//!
//! A live worker is one whose row isn't soft-deleted and whose heartbeat is
//! within the window; a dropped worker is one that isn't soft-deleted but whose
//! heartbeat has gone stale — it crashed or was killed without deregistering. A
//! gracefully stopped worker has `deleted_at` set and counts as neither.

use bestool_tamanu::ApiServerKind;

use super::{CheckContext, query_error_check};
use crate::doctor::Stat;
use crate::doctor::check::Check;

const NAME: &str = "fhir_workers";

/// Live and dropped counts read against Tamanu's own liveness window
/// (`fhir.worker.assumeDroppedAfter`, 10-minute fallback), plus the oldest live
/// heartbeat's age and the live workers' job counters summed from `metadata`.
///
/// Each worker's job counters run from the moment its process started, so the
/// sum over live workers only climbs while the same workers stay up and drops
/// when one churns out of the live set. Published as a counter, that drop reads
/// as the reset it is, and a scrape sees materialisation throughput rather than
/// a lifetime tally that sawtooths.
const SQL: &str = "\
	SELECT
		count(*) FILTER (WHERE alive) AS live,
		count(*) FILTER (WHERE NOT alive) AS dropped,
		extract(epoch FROM now() - min(updated_at) FILTER (WHERE alive))::float8 AS oldest_heartbeat_age_s,
		coalesce(sum((metadata->>'successfulJobs')::numeric) FILTER (WHERE alive), 0)::float8 AS jobs_success,
		coalesce(sum((metadata->>'failedJobs')::numeric) FILTER (WHERE alive), 0)::float8 AS jobs_failure
	FROM (
		SELECT updated_at, metadata,
			updated_at > now() - (
				SELECT coalesce(
					(setting_get('fhir.worker.assumeDroppedAfter')->>0)::interval,
					interval '10 minutes')
			) AS alive
		FROM fhir.job_workers
		WHERE deleted_at IS NULL
	) t";

pub async fn run(ctx: CheckContext) -> Check {
	if ctx.kind != ApiServerKind::Central {
		return Check::skip(
			NAME,
			"not applicable on facility server",
			"central-only check",
		);
	}
	if !ctx.config.fhir_worker_enabled() {
		return Check::skip(
			NAME,
			"FHIR worker not enabled",
			"integrations.fhir.worker.enabled is false, so no worker is expected to heartbeat",
		);
	}
	let Some(client) = ctx.db.as_ref() else {
		return Check::fail(NAME, "no DB connection", "db_connect failed");
	};

	let row = match client.query_one(SQL, &[]).await {
		Ok(r) => r,
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& (db.code() == &tokio_postgres::error::SqlState::UNDEFINED_TABLE
					|| db.code() == &tokio_postgres::error::SqlState::INVALID_SCHEMA_NAME)
			{
				return Check::skip(NAME, "fhir.job_workers table not present", "table absent");
			}
			return query_error_check(NAME, &err);
		}
	};

	let live: i64 = row.try_get("live").unwrap_or(0);
	let dropped: i64 = row.try_get("dropped").unwrap_or(0);
	let oldest_age: Option<f64> = row.try_get("oldest_heartbeat_age_s").unwrap_or(None);
	let jobs_success: f64 = row.try_get("jobs_success").unwrap_or(0.0);
	let jobs_failure: f64 = row.try_get("jobs_failure").unwrap_or(0.0);

	let summary = format!("{live} live, {dropped} dropped");
	let check = match classify(live, dropped) {
		Verdict::Fail => Check::fail(NAME, summary, "no live FHIR worker is heartbeating"),
		Verdict::Warn => Check::warning(
			NAME,
			summary,
			format!("{dropped} worker(s) stopped heartbeating without deregistering"),
		),
		Verdict::Pass => Check::pass(NAME, summary),
	};

	let mut check = check
		.with_detail("live", live)
		.with_detail("dropped", dropped)
		.with_stat(
			Stat::gauge("live", live as f64)
				.group("workers")
				.help("Live FHIR workers"),
		)
		.with_stat(
			Stat::gauge("dropped", dropped as f64)
				.group("workers")
				.help("FHIR workers with a stale heartbeat"),
		)
		.with_stat(
			Stat::counter("jobs_total", jobs_success)
				.label("result", "success")
				.group("jobs")
				.help("Jobs processed by live workers, by outcome"),
		)
		.with_stat(
			Stat::counter("jobs_total", jobs_failure)
				.label("result", "failure")
				.group("jobs")
				.help("Jobs processed by live workers, by outcome"),
		);
	if let Some(age) = oldest_age {
		check = check.with_stat(
			Stat::gauge("oldest_heartbeat_age_seconds", age)
				.help("Age of the oldest live worker's heartbeat"),
		);
	}
	check
}

enum Verdict {
	Pass,
	Warn,
	Fail,
}

/// Grade worker liveness: no live worker fails, a dropped (crashed) worker
/// warns, otherwise pass.
fn classify(live: i64, dropped: i64) -> Verdict {
	if live == 0 {
		Verdict::Fail
	} else if dropped > 0 {
		Verdict::Warn
	} else {
		Verdict::Pass
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::check::CheckStatus;
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

	fn verdict(live: i64, dropped: i64) -> &'static str {
		match classify(live, dropped) {
			Verdict::Pass => "pass",
			Verdict::Warn => "warn",
			Verdict::Fail => "fail",
		}
	}

	#[test]
	fn no_live_worker_fails() {
		assert_eq!(verdict(0, 0), "fail");
		assert_eq!(verdict(0, 2), "fail");
	}

	#[test]
	fn dropped_worker_warns_when_some_are_live() {
		assert_eq!(verdict(1, 1), "warn");
		assert_eq!(verdict(2, 3), "warn");
	}

	#[test]
	fn all_live_passes() {
		assert_eq!(verdict(1, 0), "pass");
		assert_eq!(verdict(2, 0), "pass");
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "fhir_workers");
		// Enabled or not, the outcome is a real grade or a skip — never a panic.
		assert!(matches!(
			check.status,
			CheckStatus::Pass
				| CheckStatus::Warning(_)
				| CheckStatus::Fail(_)
				| CheckStatus::Skip(_)
				| CheckStatus::Broken(_)
		));
	}
}
