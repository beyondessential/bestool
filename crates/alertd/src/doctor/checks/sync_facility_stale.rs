//! Facilities whose sync has gone stale.
//!
//! Sync runs about every 60s, so for each facility that has synced in the last
//! 48h we compute the minutes since its last successful (errorless, completed)
//! sync and tier: WARN past 10 minutes, FAIL past 30. The 48h-active guard
//! keeps decommissioned facilities from flagging.

use serde_json::{Value, json};

use super::{CheckContext, fmt_db_error};
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;

const NAME: &str = "sync_facility_stale";

const WARN_MINUTES: f64 = 10.0;
const FAIL_MINUTES: f64 = 30.0;

const SQL: &str = "WITH facility_sessions AS ( \
		SELECT jsonb_array_elements_text(parameters->'facilityIds') AS facility_id, \
			created_at, completed_at, errors \
		FROM sync_sessions WHERE parameters->>'isMobile' <> 'true' \
	), active AS ( \
		SELECT DISTINCT facility_id FROM facility_sessions \
		WHERE created_at > now() - interval '48 hours' \
	), last_success AS ( \
		SELECT facility_id, max(completed_at) AS last_successful_sync \
		FROM facility_sessions WHERE errors IS NULL AND completed_at IS NOT NULL \
		GROUP BY facility_id \
	) \
	SELECT a.facility_id, \
		ls.last_successful_sync::text AS last_successful_sync, \
		EXTRACT(EPOCH FROM (now() - ls.last_successful_sync)) / 60 AS minutes_since_success \
	FROM active a LEFT JOIN last_success ls USING (facility_id) \
	ORDER BY minutes_since_success DESC NULLS FIRST";

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
		Err(err) => return Check::fail(NAME, "query failed", fmt_db_error(&err)),
	};

	let mut warn = Vec::new();
	let mut fail = Vec::new();
	for row in &rows {
		let facility_id: String = row.try_get("facility_id").unwrap_or_default();
		let last: Option<String> = row.try_get("last_successful_sync").ok();
		// A facility that is active but has never had a successful sync (NULL
		// minutes) is as bad as a very stale one: treat it as a failure.
		let minutes: Option<f64> = row.try_get("minutes_since_success").ok();
		let entry = json!({
			"facility_id": facility_id,
			"last_successful_sync": last,
			"minutes_since_success": minutes,
		});
		match minutes {
			Some(m) if m <= WARN_MINUTES => {}
			Some(m) if m <= FAIL_MINUTES => warn.push(entry),
			_ => fail.push(entry),
		}
	}

	if warn.is_empty() && fail.is_empty() {
		return Check::pass(NAME, "all facilities syncing");
	}

	let summary = format!(
		"stale sync: {} over {}m, {} over {}m",
		fail.len(),
		FAIL_MINUTES as i64,
		warn.len(),
		WARN_MINUTES as i64
	);
	let check = if fail.is_empty() {
		Check::warning(NAME, summary, "facility sync stale")
	} else {
		Check::fail(NAME, summary, "facility sync stale")
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
		assert_eq!(check.name, "sync_facility_stale");
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
