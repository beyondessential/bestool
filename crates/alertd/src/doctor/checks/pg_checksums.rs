//! PostgreSQL data-checksums check.
//!
//! Data checksums are what makes postgres notice a page that came back from
//! storage corrupted, instead of serving the damage as if it were data. They
//! can only be turned on at `initdb --data-checksums` time, or afterwards by
//! `pg_checksums -e` against a cleanly shut down cluster, so a cluster created
//! without them stays without them until someone takes the downtime.
//!
//! Two failures, then. A cluster without checksums fails: silent corruption
//! served as data is not a condition to leave standing, even though closing the
//! gap needs a planned outage. And a cluster whose checksums have *fired* fails
//! too — that's corruption postgres has already caught on disk. Canopy's
//! per-server severity ceiling lowers either to a warning, or silences it, on a
//! deployment where the gap is accepted for now.

use super::{CheckContext, query_error_check};
use crate::doctor::{Stat, check::Check};

const NAME: &str = "pg_checksums";

/// `pg_stat_database.checksum_failures` arrived in PostgreSQL 12; older servers
/// keep no such counter, so the check reports the setting alone rather than
/// breaking on an undefined column.
const FAILURES_MIN_VERSION_NUM: i64 = 120000;

const SETTING_QUERY: &str = "
	SELECT
		current_setting('data_checksums')             AS data_checksums,
		current_setting('server_version_num')::bigint AS server_version_num
";

/// Cluster-wide, so it sums every database plus the shared-objects row: a
/// failure anywhere in the cluster is a failure of this host's storage.
const FAILURES_QUERY: &str = "
	SELECT
		coalesce(sum(checksum_failures), 0)::bigint AS checksum_failures,
		max(checksum_last_failure)::text           AS checksum_last_failure
	FROM pg_stat_database
";

#[derive(Debug, Clone)]
struct Checksums {
	/// The `data_checksums` GUC verbatim.
	setting: String,
	/// Cluster-wide failure count, `None` on a postgres too old to track it.
	failures: Option<i64>,
	/// When the most recent failure was seen, as postgres rendered it.
	last_failure: Option<String>,
}

/// Grade the cluster's checksum state.
///
/// Pure so the grading is tested without a database — corruption in particular,
/// which no test can provoke on a healthy server.
///
/// A non-zero failure count outranks the setting: the counters survive
/// checksums being turned back off, so corruption already caught stays reported
/// even once postgres has stopped looking for more. Anything other than the
/// documented `on`/`off` — a future postgres reporting an in-progress enable,
/// say — warns and quotes what it said: protection isn't confirmed, but neither
/// is it the settled absence `off` reports.
fn grade(c: &Checksums) -> Check {
	let enabled = c.setting == "on";
	let failures = c.failures.unwrap_or(0);

	let base = if failures > 0 {
		let mut reason = match &c.last_failure {
			Some(when) => format!(
				"postgres has detected {failures} data-page checksum failure(s), most recently at {when} — this cluster has corrupt pages on disk"
			),
			None => format!(
				"postgres has detected {failures} data-page checksum failure(s) — this cluster has corrupt pages on disk"
			),
		};
		if !enabled {
			reason.push_str(&format!(
				"; data_checksums is now {:?}, so further corruption goes unnoticed",
				c.setting
			));
		}
		Check::fail(NAME, format!("{failures} checksum failures"), reason)
	} else {
		match c.setting.as_str() {
			"on" => Check::pass(NAME, "data checksums enabled"),
			"off" => Check::fail(
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
	};

	let mut check = base
		.with_detail("data_checksums", c.setting.clone())
		.with_stat(
			Stat::gauge("enabled", if enabled { 1.0 } else { 0.0 })
				.help("Whether postgres data checksums are enabled"),
		);

	// Only published when postgres actually tracks it: a hardcoded zero from an
	// older server would read as "no corruption" when it means "not measured".
	if let Some(failures) = c.failures {
		check = check.with_detail("checksum_failures", failures).with_stat(
			Stat::counter("failures", failures as f64)
				.help("Postgres data-page checksum failures since the statistics were reset"),
		);
	}
	if let Some(when) = &c.last_failure {
		check = check.with_detail("checksum_last_failure", when.clone());
	}
	check
}

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::skip(
			NAME,
			"no DB connection",
			"can't read postgres settings; db_connect reports the outage",
		);
	};

	let row = match client.query_one(SETTING_QUERY, &[]).await {
		Ok(row) => row,
		Err(err) => return query_error_check(NAME, &err),
	};
	let setting = match row.try_get::<_, String>("data_checksums") {
		Ok(setting) => setting,
		// The setting exists on every supported postgres and is always text; a
		// row that doesn't decode is a fault in this check, not the cluster.
		Err(err) => return Check::broken(NAME, "row decode failed", err.to_string()),
	};
	let version_num: i64 = row.try_get("server_version_num").unwrap_or_default();

	let mut checksums = Checksums {
		setting,
		failures: None,
		last_failure: None,
	};

	if version_num >= FAILURES_MIN_VERSION_NUM {
		match client.query_one(FAILURES_QUERY, &[]).await {
			Ok(row) => {
				checksums.failures = row.try_get("checksum_failures").ok();
				checksums.last_failure = row.try_get("checksum_last_failure").ok();
			}
			Err(err) => return query_error_check(NAME, &err),
		}
	}

	grade(&checksums)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::{
		check::CheckStatus,
		checks::test_support::{central_ctx, facility_ctx},
	};

	fn state(setting: &str, failures: Option<i64>) -> Checksums {
		Checksums {
			setting: setting.into(),
			failures,
			last_failure: None,
		}
	}

	#[test]
	fn checksums_on_and_clean_passes() {
		let check = grade(&state("on", Some(0)));
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.details["data_checksums"], "on");
		assert_eq!(check.details["checksum_failures"], 0);
	}

	#[test]
	fn checksums_off_fails_and_says_how_to_fix() {
		match grade(&state("off", Some(0))).status {
			CheckStatus::Fail(reason) => assert!(
				reason.contains("pg_checksums"),
				"expected the remedy in the reason, got {reason:?}"
			),
			other => panic!("expected a failure for disabled checksums, got {other:?}"),
		}
	}

	#[test]
	fn detected_failures_fail_with_the_count_and_time() {
		let c = Checksums {
			setting: "on".into(),
			failures: Some(3),
			last_failure: Some("2026-07-30 04:05:06+00".into()),
		};
		let check = grade(&c);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains('3'), "expected the count, got {reason:?}");
				assert!(
					reason.contains("2026-07-30"),
					"expected the last-failure time, got {reason:?}"
				);
			}
			other => panic!("expected a failure for detected corruption, got {other:?}"),
		}
		assert_eq!(check.summary, "3 checksum failures");
		assert_eq!(check.details["checksum_failures"], 3);
		assert_eq!(
			check.details["checksum_last_failure"],
			"2026-07-30 04:05:06+00"
		);
	}

	/// The counters outlive checksums being turned back off, so corruption
	/// already caught must still be reported — and the reason must say that
	/// nothing is watching for more.
	#[test]
	fn failures_outrank_the_setting() {
		match grade(&state("off", Some(1))).status {
			CheckStatus::Fail(reason) => {
				assert!(
					reason.contains("corrupt pages"),
					"expected the corruption to lead, got {reason:?}"
				);
				assert!(
					reason.contains("goes unnoticed"),
					"expected the disabled state noted too, got {reason:?}"
				);
			}
			other => panic!("expected a failure, got {other:?}"),
		}
	}

	#[test]
	fn unexpected_setting_warns_and_quotes_it() {
		match grade(&state("inprogress", Some(0))).status {
			CheckStatus::Warning(reason) => assert!(
				reason.contains("inprogress"),
				"expected the raw value in the reason, got {reason:?}"
			),
			other => panic!("expected a warning for an unrecognised value, got {other:?}"),
		}
	}

	/// A pre-12 postgres tracks no failure counter. Publishing zero would read
	/// as "no corruption found" when it means "never looked", so the count is
	/// left out of both the details and the metrics.
	#[test]
	fn untracked_failures_are_not_reported_as_zero() {
		let check = grade(&state("on", None));
		assert!(matches!(check.status, CheckStatus::Pass));
		assert!(!check.details.contains_key("checksum_failures"));
		assert!(check.stats.iter().all(|s| s.name != "failures"));
	}

	#[test]
	fn metrics_cover_the_state_and_the_count() {
		let check = grade(&state("on", Some(2)));
		let enabled = check.stats.iter().find(|s| s.name == "enabled").unwrap();
		assert_eq!(enabled.value, 1.0);
		let failures = check.stats.iter().find(|s| s.name == "failures").unwrap();
		assert_eq!(failures.value, 2.0);
		// Cumulative until the stats are reset, so a counter rather than a gauge.
		assert_eq!(failures.kind, crate::doctor::stat::StatKind::Counter);

		let off = grade(&state("off", Some(0)));
		let enabled = off.stats.iter().find(|s| s.name == "enabled").unwrap();
		assert_eq!(enabled.value, 0.0);
	}

	/// Without a database the check reports nothing about the cluster: db_connect
	/// is what alerts on an outage, so this one skips rather than doubling up.
	#[tokio::test]
	async fn no_db_skips() {
		let check = run(facility_ctx()).await;
		assert!(matches!(check.status, CheckStatus::Skip(_)));
	}

	/// Against a real postgres both queries must run and grade, never break:
	/// proves the setting and `pg_stat_database` columns are valid on the server
	/// we test with.
	#[tokio::test]
	async fn reads_the_state_from_a_live_server() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = run(ctx).await;
		assert!(
			matches!(
				check.status,
				CheckStatus::Pass | CheckStatus::Warning(_) | CheckStatus::Fail(_)
			),
			"expected a graded outcome from a live server, got {:?}",
			check.status
		);
		assert!(check.details.contains_key("data_checksums"));
		// A modern test server tracks the counter, so the query must have landed.
		assert!(check.details.contains_key("checksum_failures"));
	}
}
