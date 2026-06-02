//! Shared helpers for SQL-backed checks.
//!
//! Each check fails when its query returns any rows and attaches the
//! offending rows (capped) to `details`. To avoid the generic tokio-postgres
//! row→JSON conversion and to bound memory, every query is wrapped so Postgres
//! returns one JSONB column per row, capped just past the reporting limit.

use std::sync::Arc;

use serde_json::Value;
use tokio_postgres::{Client as PgClient, types::ToSql};

use super::fmt_db_error;
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

/// Run a single wrapped query: fail (with capped rows + count) if it
/// returns any rows, else pass.
///
/// `summary_pass` is the headline shown when nothing matched;
/// `summary_fail_prefix` is prepended to the count when rows are found.
pub async fn fail_if_any_rows(
	client: &Arc<PgClient>,
	name: &'static str,
	summary_pass: &str,
	summary_fail_prefix: &str,
	sql: &str,
	params: &[&(dyn ToSql + Sync)],
) -> Check {
	match fetch_rows(client, sql, params).await {
		Ok(set) if set.is_empty() => Check::pass(name, summary_pass.to_string()),
		Ok(set) => {
			let count = set.count();
			Check::fail(
				name,
				format!("{summary_fail_prefix}{count}"),
				format!("{} matching row(s)", count),
			)
			.with_detail("rows", Value::Array(set.rows))
			.with_detail("truncated", set.truncated)
			.with_detail("count", count)
		}
		Err(err) => Check::fail(name, "query failed", fmt_db_error(&err)),
	}
}
