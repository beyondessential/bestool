//! FHIR service requests that have stayed unresolved for too long.
//!
//! Fails when a FHIR service request linked to a lab request has been
//! unresolved for over an hour.

use super::{CheckContext, util::fail_if_any_rows};
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;

const NAME: &str = "fhir_service_requests_unresolved";
const SQL: &str = "SELECT lr.display_id AS lab_request_id, \
	ROUND(EXTRACT(EPOCH FROM (NOW() - fsr.last_updated)) / 60)::text AS duration_minutes \
	FROM fhir.service_requests fsr JOIN lab_requests lr ON fsr.upstream_id = lr.id \
	WHERE fsr.resolved = FALSE AND NOW() - fsr.last_updated > INTERVAL '1 hours'";

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

	fail_if_any_rows(
		client,
		NAME,
		"no unresolved FHIR service requests",
		"unresolved FHIR service requests: ",
		SQL,
		&[],
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
		assert_eq!(check.name, "fhir_service_requests_unresolved");
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
