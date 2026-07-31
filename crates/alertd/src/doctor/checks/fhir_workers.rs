//! FHIR materialisation worker liveness.
//!
//! Tamanu's FHIR workers register a row in `fhir.job_workers` and update its
//! `updated_at` on a periodic heartbeat; job grabbing, completion, and
//! stuck-job reclamation all gate on that heartbeat being within
//! `fhir.worker.assumeDroppedAfter` (default 10 minutes). Workers that have all
//! stopped heartbeating stall materialisation even while their service processes
//! look up, which neither `tamanu_service` (process up) nor `fhir_jobs`
//! (backlog) catches.
//!
//! A live worker is one whose row isn't soft-deleted and whose heartbeat is
//! within the window; a dropped worker is one that isn't soft-deleted but whose
//! heartbeat has gone stale — it crashed or was killed without deregistering. A
//! gracefully stopped worker has `deleted_at` set and counts as neither.
//!
//! Dropped workers don't enter the verdict: their grabbed jobs return to the
//! pool for a live worker to take, and their rows sit there until the deployment
//! prunes the table, so an operator has no way to act on them.

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
	let check = match classify(live) {
		Verdict::Fail => Check::fail(NAME, summary, "no live FHIR worker is heartbeating"),
		Verdict::Pass => Check::pass(NAME, summary),
	};

	let mut check = check
		.with_detail("live", live)
		.with_detail("dropped", dropped)
		.with_stat(Stat::gauge("live", live as f64).help("Live FHIR workers"))
		.with_stat(
			Stat::counter("dropped_total", dropped as f64)
				.help("FHIR workers that stopped heartbeating without deregistering"),
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
	Fail,
}

/// Grade worker liveness: no live worker fails, otherwise pass.
fn classify(live: i64) -> Verdict {
	if live == 0 {
		Verdict::Fail
	} else {
		Verdict::Pass
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::check::CheckStatus;
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

	fn verdict(live: i64) -> &'static str {
		match classify(live) {
			Verdict::Pass => "pass",
			Verdict::Fail => "fail",
		}
	}

	#[test]
	fn no_live_worker_fails() {
		assert_eq!(verdict(0), "fail");
	}

	#[test]
	fn a_live_worker_passes() {
		assert_eq!(verdict(1), "pass");
		assert_eq!(verdict(2), "pass");
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
		// Enabled or not, the outcome is a real grade or a skip — never a panic,
		// and never a warning: liveness is pass-or-fail.
		assert!(matches!(
			check.status,
			CheckStatus::Pass
				| CheckStatus::Fail(_)
				| CheckStatus::Skip(_)
				| CheckStatus::Broken(_)
		));
	}
}
