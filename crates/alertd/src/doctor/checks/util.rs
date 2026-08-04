//! Shared helpers for SQL-backed checks.
//!
//! Each check fails when its query returns any rows and attaches the
//! offending rows (capped) to `details`. To avoid the generic tokio-postgres
//! row→JSON conversion and to bound memory, every query is wrapped so Postgres
//! returns one JSONB column per row, capped just past the reporting limit.

use std::sync::Arc;

use jiff::{Timestamp, ToSpan};
use serde_json::Value;
use tokio_postgres::{Client as PgClient, types::ToSql};

use super::query_error_check;
use crate::doctor::Stat;
use crate::doctor::check::Check;

/// Render an age in seconds as a single coarse unit, for a check summary an
/// operator reads at a glance. Rounds down, and a negative age (a clock that
/// stepped backwards between the row's timestamp and `now()`) reads as `0s`.
pub fn humanise_age(secs: i64) -> String {
	let secs = secs.max(0) as u64;
	if secs < 60 {
		format!("{secs}s")
	} else if secs < 3600 {
		format!("{}m", secs / 60)
	} else if secs < 86400 {
		format!("{}h", secs / 3600)
	} else {
		format!("{}d", secs / 86400)
	}
}

/// Rows reported in `details` are capped here; one extra row is fetched to
/// detect truncation.
const REPORT_CAP: usize = 100;
const FETCH_CAP: usize = REPORT_CAP + 1;

/// Wrap the check's SQL so Postgres hands back one JSONB column (`row`) per
/// matching row, capped at [`FETCH_CAP`], plus the exact number of matches.
///
/// `count(*) OVER ()` is evaluated across the whole subquery result before
/// `LIMIT` truncates it, so the count stays exact however many rows matched —
/// and it costs one query rather than a second round trip.
fn wrap(sql: &str) -> String {
	format!(
		"SELECT to_jsonb(sub) AS row, count(*) OVER () AS total FROM ( {sql} ) sub LIMIT {FETCH_CAP}"
	)
}

/// Outcome of running one wrapped query: the rows (capped at [`REPORT_CAP`]),
/// whether more matched than were reported, and how many matched in total.
pub struct RowSet {
	pub rows: Vec<Value>,
	/// Whether more rows matched than [`REPORT_CAP`] carries.
	pub truncated: bool,
	/// Every matching row, counted regardless of the report cap.
	pub total: u64,
}

impl RowSet {
	pub fn is_empty(&self) -> bool {
		self.total == 0
	}

	/// Number to report.
	pub fn count(&self) -> Value {
		Value::from(self.total)
	}
}

/// Run a wrapped query and collect its rows. The `to_jsonb` wrapping is
/// applied here, so callers pass the check's SQL.
pub async fn fetch_rows(
	client: &Arc<PgClient>,
	sql: &str,
	params: &[&(dyn ToSql + Sync)],
) -> Result<RowSet, tokio_postgres::Error> {
	let wrapped = wrap(sql);
	let raw = client.query(&wrapped, params).await?;
	let truncated = raw.len() > REPORT_CAP;
	// Every row carries the same window-function total; no rows means no matches.
	let total = raw
		.first()
		.map_or(0, |r| r.get::<_, i64>("total").max(0) as u64);
	let rows = raw
		.into_iter()
		.take(REPORT_CAP)
		.map(|r| r.get::<_, Value>("row"))
		.collect();
	Ok(RowSet {
		rows,
		truncated,
		total,
	})
}

/// Run a single wrapped query and tier the outcome on the number of
/// matching rows: PASS below `warn_min`, WARN at or above it, FAIL at or above
/// `fail_min`.
///
/// `summary_pass` is the headline shown when nothing crosses `warn_min`;
/// `summary_prefix` is prepended to the count for the WARN/FAIL summary.
///
/// The query's `$1` is bound to the start of the lookback window. Reported rows
/// are capped at [`REPORT_CAP`], but the count the verdict and the metric use is
/// exact.
#[expect(
	clippy::too_many_arguments,
	reason = "shared query helper; each parameter is a distinct knob the call sites set"
)]
pub async fn tiered_rows_check(
	client: &Arc<PgClient>,
	name: &'static str,
	summary_pass: &str,
	summary_prefix: &str,
	sql: &str,
	lookback_hours: i64,
	warn_min: usize,
	fail_min: usize,
) -> Check {
	let since = Timestamp::now() - lookback_hours.hours();
	match fetch_rows(client, sql, &[&since]).await {
		Ok(set) => {
			let n = set.total as usize;
			let count = set.count();
			// Emit the count as a metric on every tier, including pass. The window
			// it covers goes in the description: it's the same every sweep, but a
			// bare row count says nothing about what span produced it.
			let count_stat = Stat::gauge("count", set.total as f64)
				.help(format!("Error rows in the last {lookback_hours}h"));
			if n < warn_min {
				return Check::pass(name, summary_pass.to_string()).with_stat(count_stat);
			}
			let summary = format!("{summary_prefix}{count}");
			let reason = format!("{count} matching row(s)");
			let check = if n >= fail_min {
				Check::fail(name, summary, reason)
			} else {
				Check::warning(name, summary, reason)
			};
			check
				.with_detail("rows", Value::Array(set.rows))
				.with_detail("truncated", set.truncated)
				.with_detail("count", count)
				.with_stat(count_stat)
		}
		Err(err) => query_error_check(name, &err),
	}
}

#[cfg(test)]
mod tests {
	/// Pure count→tier decision mirroring [`tiered_rows_check`], factored so the
	/// WARN/FAIL boundaries can be asserted without a database.
	fn tier(n: usize, warn_min: usize, fail_min: usize) -> &'static str {
		if n >= fail_min {
			"fail"
		} else if n >= warn_min {
			"warning"
		} else {
			"pass"
		}
	}

	#[test]
	fn error_stream_boundaries() {
		assert_eq!(tier(0, 1, 10), "pass");
		assert_eq!(tier(1, 1, 10), "warning");
		assert_eq!(tier(9, 1, 10), "warning");
		assert_eq!(tier(10, 1, 10), "fail");
		assert_eq!(tier(100, 1, 10), "fail");
	}

	#[test]
	fn wrap_counts_outside_the_row_cap() {
		// The window function is evaluated across the subquery before LIMIT, so
		// the count survives truncation of the rows that get reported.
		let sql = super::wrap("SELECT 1");
		assert!(sql.contains("count(*) OVER () AS total"));
		assert!(sql.ends_with(&format!("LIMIT {}", super::FETCH_CAP)));
	}

	#[test]
	fn a_truncated_row_set_still_counts_exactly() {
		let set = super::RowSet {
			rows: vec![serde_json::Value::from(1); super::REPORT_CAP],
			truncated: true,
			total: 4321,
		};
		assert_eq!(set.count(), serde_json::Value::from(4321u64));
		assert!(!set.is_empty());
	}

	#[test]
	fn a_row_set_with_no_matches_is_empty() {
		let set = super::RowSet {
			rows: Vec::new(),
			truncated: false,
			total: 0,
		};
		assert!(set.is_empty());
	}
}
