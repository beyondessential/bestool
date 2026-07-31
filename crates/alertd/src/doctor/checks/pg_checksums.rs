//! PostgreSQL data-checksums check.
//!
//! Data checksums are what makes postgres notice a page that came back from
//! storage corrupted, instead of serving the damage as if it were data. They
//! can only be turned on at `initdb --data-checksums` time, or afterwards by
//! `pg_checksums -e` against a cleanly shut down cluster, so a cluster created
//! without them stays without them until someone takes the downtime.
//!
//! Graded as a warning rather than a failure: a cluster without checksums is
//! not currently unhealthy, it's carrying a latent risk that a planned outage
//! resolves. Canopy's per-server severity ceiling can lower it further on a
//! deployment where the gap is accepted.

use super::{CheckContext, query_error_check};
use crate::doctor::check::Check;

const NAME: &str = "pg_checksums";

/// Grade the value postgres reports for the `data_checksums` GUC.
///
/// Pure so the grading is tested without a database. Anything other than the
/// documented `on`/`off` — a future postgres reporting an in-progress enable,
/// say — warns and quotes what it said, rather than being read as either
/// extreme.
fn grade(setting: &str) -> Check {
	match setting {
		"on" => Check::pass(NAME, "data checksums enabled"),
		"off" => Check::warning(
			NAME,
			"data checksums disabled",
			"postgres cannot detect corrupted pages on this cluster; enable with pg_checksums -e while the cluster is shut down (or initdb --data-checksums for a new one)",
		),
		other => Check::warning(
			NAME,
			format!("data checksums {other}"),
			format!(
				"postgres reports data_checksums as {other:?}, which is neither on nor off; checksum protection is not confirmed"
			),
		),
	}
	.with_detail("data_checksums", setting)
}

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::skip(
			NAME,
			"no DB connection",
			"can't read postgres settings; db_connect reports the outage",
		);
	};

	match client
		.query_one("SELECT current_setting('data_checksums')", &[])
		.await
	{
		Ok(row) => match row.try_get::<_, String>(0) {
			Ok(setting) => grade(&setting),
			// The setting exists on every supported postgres and is always text;
			// a row that doesn't decode is a fault in this check, not the cluster.
			Err(err) => Check::broken(NAME, "row decode failed", err.to_string()),
		},
		Err(err) => query_error_check(NAME, &err),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::{
		check::CheckStatus,
		checks::test_support::{central_ctx, facility_ctx},
	};

	#[test]
	fn checksums_on_passes() {
		let check = grade("on");
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.details["data_checksums"], "on");
	}

	#[test]
	fn checksums_off_warns_and_says_how_to_fix() {
		// A cluster without checksums isn't down, so it must not alert as a
		// failure — but the operator needs the remedy in the reason.
		match grade("off").status {
			CheckStatus::Warning(reason) => assert!(
				reason.contains("pg_checksums"),
				"expected the remedy in the reason, got {reason:?}"
			),
			other => panic!("expected a warning for disabled checksums, got {other:?}"),
		}
	}

	#[test]
	fn unexpected_setting_warns_and_quotes_it() {
		match grade("inprogress").status {
			CheckStatus::Warning(reason) => assert!(
				reason.contains("inprogress"),
				"expected the raw value in the reason, got {reason:?}"
			),
			other => panic!("expected a warning for an unrecognised value, got {other:?}"),
		}
	}

	/// Without a database the check reports nothing about the cluster: db_connect
	/// is what alerts on an outage, so this one skips rather than doubling up.
	#[tokio::test]
	async fn no_db_skips() {
		let check = run(facility_ctx()).await;
		assert!(matches!(check.status, CheckStatus::Skip(_)));
	}

	/// Against a real postgres the query must run and grade, never break: proves
	/// `current_setting('data_checksums')` is valid on the server we test with.
	#[tokio::test]
	async fn reads_the_setting_from_a_live_server() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = run(ctx).await;
		assert!(
			matches!(check.status, CheckStatus::Pass | CheckStatus::Warning(_)),
			"expected a graded outcome from a live server, got {:?}",
			check.status
		);
		assert!(check.details.contains_key("data_checksums"));
	}
}
