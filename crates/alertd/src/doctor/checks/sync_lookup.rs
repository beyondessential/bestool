//! Lookup table update staleness.
//!
//! Fails when the central server hasn't recorded a successful lookup-table
//! update in over an hour.

use super::{CheckContext, util::fail_if_any_rows};
use crate::doctor::check::Check;
use bestool_tamanu::ApiServerKind;

const NAME: &str = "sync_lookup";
const SQL: &str = "SELECT key, value AS last_sync_tick, updated_at::text AS last_updated, \
	(now() - updated_at)::text AS time_since_update FROM local_system_facts \
	WHERE key = 'lastSuccessfulLookupTableUpdate' AND updated_at < now() - interval '1 hour'";

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
		"lookup table up to date",
		"lookup table stale: ",
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
		assert_eq!(check.name, "sync_lookup");
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
