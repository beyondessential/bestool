//! Facilities stuck in a sync restart loop.
//!
//! Fails when a facility has accumulated 10 or more `snapshot-for-pushing` sync
//! errors in the last hour, which indicates the sync is repeatedly restarting
//! rather than progressing.

use super::{CheckContext, util::fail_if_any_rows};
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;

const NAME: &str = "sync_restart_loop";
const SQL: &str = "SELECT jsonb_array_elements_text(parameters->'facilityIds') AS facility_id, \
	COUNT(*) AS error_count FROM sync_sessions \
	WHERE created_at > now() - interval '1 hour' AND errors IS NOT NULL \
	AND cardinality(errors) = 1 AND errors[1] LIKE '%snapshot-for-pushing%' \
	GROUP BY facility_id HAVING COUNT(*) >= 10 ORDER BY error_count DESC";

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
		"no sync restart loops",
		"facilities in sync restart loop: ",
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
		assert_eq!(check.name, "sync_restart_loop");
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
