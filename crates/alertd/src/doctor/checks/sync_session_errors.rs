//! Recent mobile and server sync-session errors, with benign-error exclusions
//! baked into the SQL.
//!
//! The window is a tight `updated_at > now() - interval '1 minute'`; the sweep
//! runs every 60s, so this still catches each error once.
//!
//! That window suits a verdict but not a graph: a scrape reads only the latest
//! sweep, so at munin's five-minute interval four minutes in five would never be
//! seen. The published metric is therefore a running total the daemon
//! accumulates a window at a time, which a scrape derives its own rate from.
//!
//! Each query returns one row per session. The facility list rides along as the
//! `facilityIds` array rather than being expanded into a row per facility: a
//! set-returning function in the target list cross-joins, which would count a
//! session spanning several facilities once per facility, and would drop a
//! session whose `parameters` carry no `facilityIds` at all.

use std::sync::atomic::{AtomicU64, Ordering};

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

/// Errors seen since this process started, one stream each.
///
/// Postgres can only be asked what happened in a window; there is no cheap
/// cumulative total to read, since no index covers `errors IS NOT NULL` across
/// the whole of `sync_sessions`. So the daemon does the accumulating: each
/// sweep's window is added to a running total that a scrape derives its own rate
/// from.
static MOBILE_SEEN: AtomicU64 = AtomicU64::new(0);
static SERVER_SEEN: AtomicU64 = AtomicU64::new(0);

/// Add this sweep's count to a running total and read the total back.
fn accumulate(seen: &AtomicU64, found: u64) -> u64 {
	seen.fetch_add(found, Ordering::Relaxed) + found
}

/// Attach the running totals, which stand independent of the verdict: a sweep
/// that finds nothing still reports every error counted before it.
fn with_error_counters(check: Check, mobile_seen: u64, server_seen: u64) -> Check {
	check
		.with_stat(
			Stat::counter("errors_total", mobile_seen as f64)
				.label("stream", "mobile")
				.group("errors")
				.help("Sync-session errors seen"),
		)
		.with_stat(
			Stat::counter("errors_total", server_seen as f64)
				.label("stream", "server")
				.group("errors")
				.help("Sync-session errors seen"),
		)
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

	let mobile_seen = accumulate(&MOBILE_SEEN, mobile.total);
	let server_seen = accumulate(&SERVER_SEEN, server.total);

	if mobile.is_empty() && server.is_empty() {
		return with_error_counters(
			Check::pass(NAME, "no recent sync session errors"),
			mobile_seen,
			server_seen,
		);
	}

	let (mobile_count, mobile_truncated) = (mobile.count(), mobile.truncated);
	let (server_count, server_truncated) = (server.count(), server.truncated);
	let total = (mobile.total + server.total) as usize;

	let summary = format!("sync session errors: {mobile_count} mobile, {server_count} server");
	let reason = "recent sync session error(s)";
	let check = if total >= FAIL_ERRORS {
		Check::fail(NAME, summary, reason)
	} else {
		Check::warning(NAME, summary, reason)
	};
	with_error_counters(
		check
			.with_detail("mobile", Value::Array(mobile.rows))
			.with_detail("mobile_count", mobile_count)
			.with_detail("mobile_truncated", mobile_truncated)
			.with_detail("server", Value::Array(server.rows))
			.with_detail("server_count", server_count)
			.with_detail("server_truncated", server_truncated),
		mobile_seen,
		server_seen,
	)
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::AtomicU64;

	use super::{MOBILE_SQL, SERVER_SQL, accumulate};
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
	fn accumulate_sums_successive_windows() {
		let seen = AtomicU64::new(0);
		assert_eq!(accumulate(&seen, 3), 3);
		assert_eq!(accumulate(&seen, 4), 7);
		// a quiet window leaves the total where it was
		assert_eq!(accumulate(&seen, 0), 7);
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
