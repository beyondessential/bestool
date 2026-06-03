//! Lookup table update staleness.
//!
//! The lookup table refreshes roughly every 20s, so tier on minutes of
//! staleness: WARN past 2 minutes, FAIL past 5. If the tracking row is absent,
//! treat the lookup as not tracked and pass.

use super::{CheckContext, query_error_check};
use crate::doctor::check::{Check, CheckStatus};
use bestool_tamanu::ApiServerKind;

const NAME: &str = "sync_lookup";
const SQL: &str = "SELECT value AS last_sync_tick, updated_at::text AS last_updated, \
	EXTRACT(EPOCH FROM (now() - updated_at))::bigint AS age_seconds \
	FROM local_system_facts WHERE key = 'lastSuccessfulLookupTableUpdate'";

const WARN_SECS: i64 = 2 * 60;
const FAIL_SECS: i64 = 5 * 60;

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

	let row = match client.query_opt(SQL, &[]).await {
		Ok(Some(r)) => r,
		Ok(None) => return Check::pass(NAME, "lookup table not tracked"),
		Err(err) => return query_error_check(NAME, &err),
	};

	let last_sync_tick: Option<String> = row.try_get("last_sync_tick").ok();
	let last_updated: Option<String> = row.try_get("last_updated").ok();
	let age_seconds: i64 = row.try_get("age_seconds").unwrap_or(0);

	let summary = format!("lookup table updated {}m ago", age_seconds / 60);
	let check = match tier(age_seconds) {
		CheckStatus::Fail(_) => Check::fail(
			NAME,
			summary,
			format!("lookup table stale: {age_seconds}s since last update"),
		),
		CheckStatus::Warning(_) => Check::warning(
			NAME,
			summary,
			format!("lookup table stale: {age_seconds}s since last update"),
		),
		_ => Check::pass(NAME, "lookup table up to date"),
	};

	let mut check = check.with_detail("age_seconds", age_seconds);
	if let Some(tick) = last_sync_tick {
		check = check.with_detail("last_sync_tick", tick);
	}
	if let Some(updated) = last_updated {
		check = check.with_detail("last_updated", updated);
	}
	check
}

/// Tier on seconds since the lookup table last updated.
fn tier(seconds: i64) -> CheckStatus {
	if seconds > FAIL_SECS {
		CheckStatus::Fail(String::new())
	} else if seconds > WARN_SECS {
		CheckStatus::Warning(String::new())
	} else {
		CheckStatus::Pass
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

	#[test]
	fn tier_boundaries() {
		assert!(matches!(tier(0), CheckStatus::Pass));
		assert!(matches!(tier(120), CheckStatus::Pass));
		assert!(matches!(tier(121), CheckStatus::Warning(_)));
		assert!(matches!(tier(300), CheckStatus::Warning(_)));
		assert!(matches!(tier(301), CheckStatus::Fail(_)));
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "sync_lookup");
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
