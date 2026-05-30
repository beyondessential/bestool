//! Facilities whose sync has gone stale.
//!
//! Flags facilities that synced in the last 48h but have had no completion in
//! the last 30m, as well as facilities whose last successful sync was over an
//! hour ago.

use serde_json::Value;

use super::{CheckContext, util::fetch_rows};
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;

const NAME: &str = "sync_facility_stale";

const NOT_SYNCING_SQL: &str = "with sync_sessions_with_facility_id as ( \
		select created_at, completed_at, \
			jsonb_array_elements_text(parameters->'facilityIds') as facility_id \
		from sync_sessions where parameters->>'isMobile' <> 'true' \
	) \
	select distinct facility_id from sync_sessions_with_facility_id \
	where created_at > current_timestamp - '48 hours'::interval \
	except \
	select facility_id from sync_sessions_with_facility_id \
	where completed_at > current_timestamp - '30 minutes'::interval \
	group by facility_id order by facility_id";

const NO_RECENT_SUCCESS_SQL: &str = "SELECT facility_id, last_successful_sync FROM ( \
		SELECT facility_id, max(completed_at) as last_successful_sync FROM ( \
			SELECT jsonb_array_elements_text(parameters->'facilityIds') as facility_id, completed_at \
			FROM sync_sessions WHERE errors IS NULL \
		) AS successful_syncs GROUP BY facility_id \
	) AS last_successful_facility_syncs \
	WHERE last_successful_sync < now() - interval '1 hour'";

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

	let not_syncing = match fetch_rows(client, NOT_SYNCING_SQL, &[]).await {
		Ok(set) => set,
		Err(err) => return Check::fail(NAME, "query failed", super::fmt_db_error(&err)),
	};
	let no_recent_success = match fetch_rows(client, NO_RECENT_SUCCESS_SQL, &[]).await {
		Ok(set) => set,
		Err(err) => return Check::fail(NAME, "query failed", super::fmt_db_error(&err)),
	};

	if not_syncing.is_empty() && no_recent_success.is_empty() {
		return Check::pass(NAME, "all facilities syncing");
	}

	let (not_syncing_count, not_syncing_truncated) = (not_syncing.count(), not_syncing.truncated);
	let (no_recent_count, no_recent_truncated) =
		(no_recent_success.count(), no_recent_success.truncated);

	let check = Check::fail(
		NAME,
		format!(
			"stale sync: {not_syncing_count} not syncing, {no_recent_count} with no recent success"
		),
		"facility sync stale",
	);
	check
		.with_detail("not_syncing", Value::Array(not_syncing.rows))
		.with_detail("not_syncing_count", not_syncing_count)
		.with_detail("not_syncing_truncated", not_syncing_truncated)
		.with_detail("no_recent_success", Value::Array(no_recent_success.rows))
		.with_detail("no_recent_success_count", no_recent_count)
		.with_detail("no_recent_success_truncated", no_recent_truncated)
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
		assert_eq!(check.name, "sync_facility_stale");
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
