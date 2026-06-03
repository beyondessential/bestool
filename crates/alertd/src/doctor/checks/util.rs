//! Shared helpers for SQL-backed checks.
//!
//! Each check fails when its query returns any rows and attaches the
//! offending rows (capped) to `details`. To avoid the generic tokio-postgres
//! row→JSON conversion and to bound memory, every query is wrapped so Postgres
//! returns one JSONB column per row, capped just past the reporting limit.

use std::sync::Arc;

use serde_json::Value;
use tokio_postgres::{Client as PgClient, types::ToSql};

use super::query_error_check;
use crate::doctor::check::Check;

/// Rows reported in `details` are capped here; one extra row is fetched to
/// detect truncation.
const REPORT_CAP: usize = 100;
const FETCH_CAP: usize = REPORT_CAP + 1;

/// Wrap the check's SQL so Postgres hands back one JSONB column (`row`) per
/// matching row, capped at [`FETCH_CAP`].
fn wrap(sql: &str) -> String {
	format!("SELECT to_jsonb(sub) AS row FROM ( {sql} ) sub LIMIT {FETCH_CAP}")
}

/// Outcome of running one wrapped query: the rows (capped at
/// [`REPORT_CAP`]) and whether more existed than were reported.
pub struct RowSet {
	pub rows: Vec<Value>,
	pub truncated: bool,
}

impl RowSet {
	pub fn is_empty(&self) -> bool {
		self.rows.is_empty()
	}

	/// Number to report: the exact count, or `"100+"` when truncated.
	pub fn count(&self) -> Value {
		if self.truncated {
			Value::from(format!("{REPORT_CAP}+"))
		} else {
			Value::from(self.rows.len())
		}
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
	let rows = raw
		.into_iter()
		.take(REPORT_CAP)
		.map(|r| r.get::<_, Value>("row"))
		.collect();
	Ok(RowSet { rows, truncated })
}

/// Run a single wrapped query and tier the outcome on the number of
/// matching rows: PASS below `warn_min`, WARN at or above it, FAIL at or above
/// `fail_min`.
///
/// `summary_pass` is the headline shown when nothing crosses `warn_min`;
/// `summary_prefix` is prepended to the count for the WARN/FAIL summary.
///
/// Rows are capped at [`REPORT_CAP`] (reported as `"100+"`), which is enough to
/// distinguish the small WARN/FAIL boundaries the error-stream checks use.
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
	params: &[&(dyn ToSql + Sync)],
	warn_min: usize,
	fail_min: usize,
) -> Check {
	match fetch_rows(client, sql, params).await {
		Ok(set) => {
			// `truncated` means there were more than REPORT_CAP rows, which is
			// well past any realistic fail_min, so treat it as the cap.
			let n = if set.truncated {
				REPORT_CAP + 1
			} else {
				set.rows.len()
			};
			let count = set.count();
			if n < warn_min {
				return Check::pass(name, summary_pass.to_string());
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
}
