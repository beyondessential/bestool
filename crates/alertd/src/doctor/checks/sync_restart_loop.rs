//! Facilities stuck in a sync restart loop.
//!
//! Counts `snapshot-for-pushing` sync errors per facility in the last hour,
//! which indicates sync repeatedly restarting rather than progressing. WARN at
//! 5 restarts/hr, FAIL at 10.

use serde_json::{Value, json};

use super::{CheckContext, query_error_check};
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;

const NAME: &str = "sync_restart_loop";

const WARN_RESTARTS: i64 = 5;
const FAIL_RESTARTS: i64 = 10;

const SQL: &str = "SELECT jsonb_array_elements_text(parameters->'facilityIds') AS facility_id, \
	COUNT(*) AS error_count FROM sync_sessions \
	WHERE created_at > now() - interval '1 hour' AND errors IS NOT NULL \
	AND cardinality(errors) = 1 AND errors[1] LIKE '%snapshot-for-pushing%' \
	GROUP BY facility_id HAVING COUNT(*) >= 5 ORDER BY error_count DESC";

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

	let mut warn = Vec::new();
	let mut fail = Vec::new();
	for row in &rows {
		let facility_id: String = row.try_get("facility_id").unwrap_or_default();
		let error_count: i64 = row.try_get("error_count").unwrap_or(0);
		let entry = json!({ "facility_id": facility_id, "error_count": error_count });
		if error_count >= FAIL_RESTARTS {
			fail.push(entry);
		} else if error_count >= WARN_RESTARTS {
			warn.push(entry);
		}
	}

	if warn.is_empty() && fail.is_empty() {
		return Check::pass(NAME, "no sync restart loops");
	}

	let summary = format!(
		"sync restart loops: {} over {FAIL_RESTARTS}/hr, {} over {WARN_RESTARTS}/hr",
		fail.len(),
		warn.len()
	);
	let check = if fail.is_empty() {
		Check::warning(NAME, summary, "facilities in sync restart loop")
	} else {
		Check::fail(NAME, summary, "facilities in sync restart loop")
	};
	check
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
		assert_eq!(check.name, "sync_restart_loop");
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
