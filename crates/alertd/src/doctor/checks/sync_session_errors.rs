//! Recent mobile and server sync-session errors, with benign-error exclusions
//! baked into the SQL.
//!
//! The window is a tight `updated_at > now() - interval '1 minute'`; the sweep
//! runs every 60s, so this still catches each error once.

use serde_json::Value;

use super::{CheckContext, util::fetch_rows};
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;

const NAME: &str = "sync_session_errors";

const FAIL_ERRORS: usize = 10;

const MOBILE_SQL: &str = "SELECT id, errors::text, \
	jsonb_array_elements_text(parameters->'facilityIds') AS facility_id, \
	created_at::text AS created, (completed_at - created_at)::text AS duration \
	FROM sync_sessions \
	WHERE updated_at > now() - interval '1 minute' \
	AND parameters->>'isMobile' = 'true' \
	AND errors IS NOT NULL \
	AND errors <> ARRAY['Session marked as completed due to its device reconnecting'] \
	AND errors <> ARRAY['could not serialize access due to concurrent update'] \
	ORDER BY created_at DESC";

const SERVER_SQL: &str = "SELECT id, errors::text, \
	jsonb_array_elements_text(parameters->'facilityIds') AS facility_id, \
	created_at::text AS created, (completed_at - created_at)::text AS duration \
	FROM sync_sessions \
	WHERE updated_at > now() - interval '1 minute' \
	AND parameters->>'isMobile' IS DISTINCT FROM 'true' \
	AND errors IS NOT NULL \
	AND errors <> ARRAY['could not serialize access due to concurrent update'] \
	AND NOT (cardinality(errors) = 1 AND errors[1] LIKE '%snapshot-for-pushing%') \
	ORDER BY created_at DESC";

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

	let mobile = match fetch_rows(client, MOBILE_SQL, &[]).await {
		Ok(set) => set,
		Err(err) => return Check::fail(NAME, "query failed", super::fmt_db_error(&err)),
	};
	let server = match fetch_rows(client, SERVER_SQL, &[]).await {
		Ok(set) => set,
		Err(err) => return Check::fail(NAME, "query failed", super::fmt_db_error(&err)),
	};

	if mobile.is_empty() && server.is_empty() {
		return Check::pass(NAME, "no recent sync session errors");
	}

	let (mobile_count, mobile_truncated) = (mobile.count(), mobile.truncated);
	let (server_count, server_truncated) = (server.count(), server.truncated);

	// Truncation means well over FAIL_ERRORS rows, so saturate the total there.
	let total = if mobile.truncated || server.truncated {
		FAIL_ERRORS
	} else {
		mobile.rows.len() + server.rows.len()
	};

	let summary = format!("sync session errors: {mobile_count} mobile, {server_count} server");
	let reason = "recent sync session error(s)";
	let check = if total >= FAIL_ERRORS {
		Check::fail(NAME, summary, reason)
	} else {
		Check::warning(NAME, summary, reason)
	};
	check
		.with_detail("mobile", Value::Array(mobile.rows))
		.with_detail("mobile_count", mobile_count)
		.with_detail("mobile_truncated", mobile_truncated)
		.with_detail("server", Value::Array(server.rows))
		.with_detail("server_count", server_count)
		.with_detail("server_truncated", server_truncated)
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
		assert_eq!(check.name, "sync_session_errors");
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
