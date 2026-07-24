//! FHIR service requests that have stayed unresolved for too long.
//!
//! Lists FHIR service requests linked to a lab request that have been
//! unresolved for over an hour, tiering on the longest outstanding duration:
//! WARN past 1h, FAIL past 6h.

use super::{CheckContext, query_error_check};
use crate::doctor::Stat;
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;
use serde_json::{Value, json};

const NAME: &str = "fhir_service_requests_unresolved";

const WARN_MINUTES: f64 = 60.0;
const FAIL_MINUTES: f64 = 6.0 * 60.0;

const SQL: &str = "SELECT lr.display_id AS lab_request_id, \
	EXTRACT(EPOCH FROM (NOW() - fsr.last_updated)) / 60 AS duration_minutes \
	FROM fhir.service_requests fsr JOIN lab_requests lr ON fsr.upstream_id = lr.id \
	WHERE fsr.resolved = FALSE AND NOW() - fsr.last_updated > INTERVAL '1 hours' \
	ORDER BY duration_minutes DESC";

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

	let rows = match client.query(SQL, &[]).await {
		Ok(r) => r,
		Err(err) => return query_error_check(NAME, &err),
	};

	if rows.is_empty() {
		return Check::pass(NAME, "no unresolved FHIR service requests")
			.with_stat(Stat::gauge("fail", 0.0).help("Requests unresolved past the fail threshold"))
			.with_stat(
				Stat::gauge("warn", 0.0).help("Requests unresolved past the warn threshold"),
			);
	}

	let mut warn = Vec::new();
	let mut fail = Vec::new();
	for row in &rows {
		let lab_request_id: Option<String> = row.try_get("lab_request_id").ok();
		let minutes: f64 = row.try_get("duration_minutes").unwrap_or(0.0);
		let entry = json!({
			"lab_request_id": lab_request_id,
			"duration_minutes": minutes.round() as i64,
		});
		if minutes > FAIL_MINUTES {
			fail.push(entry);
		} else if minutes > WARN_MINUTES {
			warn.push(entry);
		}
	}

	if warn.is_empty() && fail.is_empty() {
		return Check::pass(NAME, "no unresolved FHIR service requests")
			.with_stat(Stat::gauge("fail", 0.0).help("Requests unresolved past the fail threshold"))
			.with_stat(
				Stat::gauge("warn", 0.0).help("Requests unresolved past the warn threshold"),
			);
	}

	let (fail_n, warn_n) = (fail.len(), warn.len());
	let summary = format!(
		"unresolved FHIR service requests: {} over 6h, {} over 1h",
		fail.len(),
		warn.len()
	);
	let check = if fail.is_empty() {
		Check::warning(NAME, summary, "unresolved FHIR service request(s)")
	} else {
		Check::fail(NAME, summary, "unresolved FHIR service request(s)")
	};
	check
		.with_stat(
			Stat::gauge("fail", fail_n as f64).help("Requests unresolved past the fail threshold"),
		)
		.with_stat(
			Stat::gauge("warn", warn_n as f64).help("Requests unresolved past the warn threshold"),
		)
		.with_detail("fail", Value::Array(fail))
		.with_detail("warn", Value::Array(warn))
}

#[cfg(test)]
mod tests {
	use crate::doctor::check::CheckStatus;
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "fhir_service_requests_unresolved");
		assert!(matches!(
			check.status,
			CheckStatus::Pass | CheckStatus::Warning(_) | CheckStatus::Fail(_)
		));
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
