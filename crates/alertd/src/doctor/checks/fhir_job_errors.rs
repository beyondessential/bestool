//! FHIR jobs that recorded an error within the lookback window.
//!
//! Distinct from `fhir_jobs`, which measures live queue depth: this surfaces
//! individual jobs that errored recently.

use jiff::{Timestamp, ToSpan};

use super::{CheckContext, util::fail_if_any_rows};
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;

const NAME: &str = "fhir_job_errors";
const SQL: &str =
	"SELECT * FROM fhir.jobs WHERE error IS NOT NULL AND created_at > $1 ORDER BY created_at DESC";

// Lookback window for recent-error checks.
const LOOKBACK_HOURS: i64 = 1;

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

	let since = Timestamp::now() - LOOKBACK_HOURS.hours();
	fail_if_any_rows(
		client,
		"fhir_job_errors",
		"no recent FHIR job errors",
		"FHIR job errors: ",
		SQL,
		&[&since],
	)
	.await
}

#[cfg(test)]
mod tests {
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "fhir_job_errors");
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
