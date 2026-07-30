//! Recent mobile and server sync-session errors, with benign-error exclusions
//! baked into the SQL.
//!
//! The window is a tight `updated_at > now() - interval '1 minute'`; the sweep
//! runs every 60s, so this still catches each error once.
//!
//! Each query returns one row per session. The facility list rides along as the
//! `facilityIds` array rather than being expanded into a row per facility: a
//! set-returning function in the target list cross-joins, which would count a
//! session spanning several facilities once per facility, and would drop a
//! session whose `parameters` carry no `facilityIds` at all.

use serde_json::Value;

use super::{CheckContext, util::fetch_rows};
use crate::doctor::Stat;
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;

const NAME: &str = "sync_session_errors";

const FAIL_ERRORS: usize = 10;

const MOBILE_SQL: &str = "SELECT id, errors::text, \
	parameters->'facilityIds' AS facility_ids, \
	created_at::text AS created, (completed_at - created_at)::text AS duration \
	FROM sync_sessions \
	WHERE updated_at > now() - interval '1 minute' \
	AND parameters->>'isMobile' = 'true' \
	AND errors IS NOT NULL \
	AND errors <> ARRAY['Session marked as completed due to its device reconnecting'] \
	AND errors <> ARRAY['could not serialize access due to concurrent update'] \
	ORDER BY created_at DESC";

const SERVER_SQL: &str = "SELECT id, errors::text, \
	parameters->'facilityIds' AS facility_ids, \
	created_at::text AS created, (completed_at - created_at)::text AS duration \
	FROM sync_sessions \
	WHERE updated_at > now() - interval '1 minute' \
	AND parameters->>'isMobile' IS DISTINCT FROM 'true' \
	AND errors IS NOT NULL \
	AND errors <> ARRAY['could not serialize access due to concurrent update'] \
	AND NOT (cardinality(errors) = 1 AND errors[1] LIKE '%snapshot-for-pushing%') \
	ORDER BY created_at DESC";

/// Errored sessions the verdict tiers on.
///
/// A truncated row set means there were more sessions than the report cap, well
/// past [`FAIL_ERRORS`], so the total saturates there rather than reporting the
/// cap as if it were the real figure.
fn error_total(
	mobile_n: usize,
	mobile_truncated: bool,
	server_n: usize,
	server_truncated: bool,
) -> usize {
	if mobile_truncated || server_truncated {
		FAIL_ERRORS
	} else {
		mobile_n + server_n
	}
}

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
		Err(err) => return super::query_error_check(NAME, &err),
	};
	let server = match fetch_rows(client, SERVER_SQL, &[]).await {
		Ok(set) => set,
		Err(err) => return super::query_error_check(NAME, &err),
	};

	if mobile.is_empty() && server.is_empty() {
		return Check::pass(NAME, "no recent sync session errors")
			.with_stat(Stat::gauge("mobile_errors", 0.0).help("Recent mobile sync-session errors"))
			.with_stat(
				Stat::gauge("server_errors", 0.0).help("Recent server sync-session errors"),
			);
	}

	let (mobile_count, mobile_truncated) = (mobile.count(), mobile.truncated);
	let (server_count, server_truncated) = (server.count(), server.truncated);
	// Row vecs are capped at the report cap, so these saturate there on truncation.
	let (mobile_n, server_n) = (mobile.rows.len(), server.rows.len());

	let total = error_total(mobile_n, mobile_truncated, server_n, server_truncated);

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
		.with_stat(
			Stat::gauge("mobile_errors", mobile_n as f64).help("Recent mobile sync-session errors"),
		)
		.with_stat(
			Stat::gauge("server_errors", server_n as f64).help("Recent server sync-session errors"),
		)
}

#[cfg(test)]
mod tests {
	use super::{FAIL_ERRORS, MOBILE_SQL, SERVER_SQL, error_total};
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

	#[test]
	fn queries_return_one_row_per_session() {
		// Expanding facilityIds in the target list cross-joins, so a session
		// would be counted once per facility and a session carrying no
		// facilityIds would not appear at all.
		for sql in [MOBILE_SQL, SERVER_SQL] {
			assert!(!sql.contains("jsonb_array_elements"));
			assert!(sql.contains("parameters->'facilityIds' AS facility_ids"));
		}
	}

	#[test]
	fn error_total_sums_both_streams() {
		assert_eq!(error_total(0, false, 0, false), 0);
		assert_eq!(error_total(3, false, 4, false), 7);
	}

	#[test]
	fn error_total_saturates_when_either_stream_truncates() {
		assert_eq!(error_total(100, true, 0, false), FAIL_ERRORS);
		assert_eq!(error_total(0, false, 100, true), FAIL_ERRORS);
	}

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
