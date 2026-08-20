//! Blobs the store repaired from their own parity.
//!
//! No content was lost — that is what separates this from `blob_integrity`.
//! Repair happening often enough to report means the storage under the blob
//! store is starting to fail, so the response is to plan replacing the media
//! rather than to recover anything. Read it alongside the host disk checks.
//!
//! A single correction on one blob is the feature working as intended, so the
//! spread matters more than the raw repair count: WARN once 3 distinct blobs
//! have been repaired within a week, FAIL at 10 within a day, or at 5 when that
//! is more than three times the week's daily average.

use serde_json::Value;
use tokio_postgres::error::SqlState;

use super::{CheckContext, query_error_check};
use crate::doctor::Stat;
use crate::doctor::check::Check;

const NAME: &str = "blob_correction_rate";

const WARN_BLOBS_7D: i64 = 3;
const FAIL_BLOBS_24H: i64 = 10;
const RISING_MIN_24H: i64 = 5;
const RISING_FACTOR: f64 = 3.0;

const SQL: &str = "SELECT \
	count(*) FILTER (WHERE last_corrected_at > now() - interval '24 hours') AS blobs_24h, \
	count(*) FILTER (WHERE last_corrected_at > now() - interval '7 days') AS blobs_7d, \
	count(*) AS blobs_corrected, \
	coalesce(sum(correction_count), 0) AS corrections_total, \
	max(last_corrected_at)::text AS most_recent \
	FROM blobs WHERE correction_count > 0 AND deleted_at IS NULL";

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_ref() else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let row = match client.query_one(SQL, &[]).await {
		Ok(r) => r,
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& matches!(
					db.code(),
					&SqlState::UNDEFINED_TABLE | &SqlState::UNDEFINED_COLUMN
				) {
				return Check::skip(
					NAME,
					"blob error correction not available",
					"this Tamanu has no blob parity columns",
				);
			}
			return query_error_check(NAME, &err);
		}
	};

	let blobs_24h: i64 = row.try_get("blobs_24h").unwrap_or(0);
	let blobs_7d: i64 = row.try_get("blobs_7d").unwrap_or(0);
	let blobs_corrected: i64 = row.try_get("blobs_corrected").unwrap_or(0);
	let corrections_total: i64 = row.try_get("corrections_total").unwrap_or(0);
	let most_recent: Option<String> = row.try_get("most_recent").unwrap_or(None);

	let summary = if blobs_corrected == 0 {
		"no blobs repaired from parity".to_string()
	} else {
		format!(
			"blobs repaired from parity: {blobs_24h} in 24h, {blobs_7d} in 7d, \
			{blobs_corrected} in total over {corrections_total} repair(s)"
		)
	};
	let check = match classify(blobs_24h, blobs_7d) {
		Verdict::Pass => Check::pass(NAME, summary),
		Verdict::Warn(reason) => Check::warning(NAME, summary, reason),
		Verdict::Fail(reason) => Check::fail(NAME, summary, reason),
	};

	let mut check = check
		.with_detail("blobs_24h", blobs_24h)
		.with_detail("blobs_7d", blobs_7d)
		.with_detail("blobs_corrected", blobs_corrected)
		.with_detail("corrections_total", corrections_total)
		.with_stat(
			Stat::gauge("blobs_24h", blobs_24h as f64)
				.group("corrected")
				.help("Blobs repaired from parity within the window"),
		)
		.with_stat(
			Stat::gauge("blobs_7d", blobs_7d as f64)
				.group("corrected")
				.help("Blobs repaired from parity within the window"),
		)
		.with_stat(
			Stat::gauge("blobs_corrected", blobs_corrected as f64)
				.help("Blobs that have ever been repaired from parity"),
		)
		.with_stat(
			Stat::gauge("corrections_total", corrections_total as f64)
				.help("Repairs made from parity across the store"),
		);
	if let Some(most_recent) = most_recent {
		check = check.with_detail("most_recent", Value::from(most_recent));
	}
	check
}

enum Verdict {
	Pass,
	Warn(String),
	Fail(String),
}

/// Grade the store on how widely repair is spread and whether it is speeding up.
///
/// Both counts are distinct blobs, not repairs: repeated repair of one blob is a
/// single bad region, while the same number of repairs across separate blobs is
/// the substrate going and the more serious reading.
///
/// The flat 24h threshold is what keeps a store that has already plateaued at a
/// high repair rate failing, once the acceleration test has nothing left to see
/// inside its own window.
fn classify(blobs_24h: i64, blobs_7d: i64) -> Verdict {
	if blobs_24h >= FAIL_BLOBS_24H {
		Verdict::Fail(format!(
			"{blobs_24h} blobs repaired from parity in the last 24 hours"
		))
	} else if blobs_24h >= RISING_MIN_24H
		&& blobs_24h as f64 > RISING_FACTOR * blobs_7d as f64 / 7.0
	{
		Verdict::Fail(format!(
			"repair is accelerating: {blobs_24h} blobs in the last 24 hours against {blobs_7d} over 7 days"
		))
	} else if blobs_7d >= WARN_BLOBS_7D {
		Verdict::Warn(format!(
			"{blobs_7d} distinct blobs repaired from parity in the last 7 days"
		))
	} else {
		Verdict::Pass
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::check::CheckStatus;
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

	fn verdict(blobs_24h: i64, blobs_7d: i64) -> &'static str {
		match classify(blobs_24h, blobs_7d) {
			Verdict::Pass => "pass",
			Verdict::Warn(_) => "warn",
			Verdict::Fail(_) => "fail",
		}
	}

	#[test]
	fn an_untouched_store_passes() {
		assert_eq!(verdict(0, 0), "pass");
	}

	#[test]
	fn the_odd_repair_passes() {
		// One blob repaired over a week is the parity doing its job.
		assert_eq!(verdict(0, 1), "pass");
		assert_eq!(verdict(1, 1), "pass");
		assert_eq!(verdict(2, 2), "pass");
	}

	#[test]
	fn spread_across_several_blobs_warns() {
		assert_eq!(verdict(0, 3), "warn");
		assert_eq!(verdict(3, 3), "warn");
		assert_eq!(verdict(4, 9), "warn");
	}

	#[test]
	fn a_steady_trickle_stays_a_warning() {
		// Five a day for a week is spread, but flat: nothing has changed today.
		assert_eq!(verdict(5, 35), "warn");
		assert_eq!(verdict(9, 63), "warn");
	}

	#[test]
	fn a_rising_rate_fails() {
		// Today carries far more than the week's daily average.
		assert_eq!(verdict(5, 5), "fail");
		assert_eq!(verdict(8, 18), "fail");
	}

	#[test]
	fn a_rise_below_the_floor_stays_a_warning() {
		// Four blobs in a day is a step up from nothing, but too few to escalate.
		assert_eq!(verdict(4, 4), "warn");
	}

	#[test]
	fn a_high_flat_rate_fails() {
		// Plateaued at ten a day: the acceleration test sees nothing, so the flat
		// threshold has to carry it.
		assert_eq!(verdict(10, 70), "fail");
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "blob_correction_rate");
		assert!(
			!matches!(check.status, CheckStatus::Broken(_)),
			"a Tamanu without the parity columns should skip, not break: {:?}",
			check.to_wire()["result"]
		);
	}

	#[tokio::test]
	async fn skips_without_a_database() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
