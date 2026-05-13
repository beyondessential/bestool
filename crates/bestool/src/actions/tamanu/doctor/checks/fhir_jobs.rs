use serde_json::{Map, Value};

use super::CheckContext;
use crate::actions::tamanu::doctor::check::Check;

const WARN_ERROR_PCT: f64 = 5.0;
const FAIL_ERROR_PCT: f64 = 50.0;

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::fail("fhir_jobs", "no DB connection", "db_connect failed");
	};

	let query = r#"SELECT status, count(*)::bigint AS n FROM fhir.jobs GROUP BY status"#;
	let rows = match client.query(query, &[]).await {
		Ok(rows) => rows,
		Err(err) => {
			let msg = err.to_string();
			if msg.contains("fhir.jobs") && msg.contains("does not exist") {
				return Check::pass("fhir_jobs", "fhir.jobs table not present")
					.with_detail("skipped", true);
			}
			return Check::fail("fhir_jobs", "query failed", msg);
		}
	};

	let mut by_status: Map<String, Value> = Map::new();
	let mut total: i64 = 0;
	let mut errored: i64 = 0;
	for row in rows {
		let status: String = row.try_get("status").unwrap_or_else(|_| "unknown".into());
		let n: i64 = row.try_get("n").unwrap_or(0);
		total += n;
		if status.eq_ignore_ascii_case("errored") || status.eq_ignore_ascii_case("error") {
			errored += n;
		}
		by_status.insert(status, Value::from(n));
	}

	let pct = if total > 0 {
		(errored as f64 / total as f64) * 100.0
	} else {
		0.0
	};
	let summary = format!("{total} jobs, {errored} errored ({pct:.1}%)");

	let check = if total > 0 && pct >= FAIL_ERROR_PCT {
		Check::fail(
			"fhir_jobs",
			summary.clone(),
			format!("≥{FAIL_ERROR_PCT}% errored"),
		)
	} else if errored > 0 && pct >= WARN_ERROR_PCT {
		Check::warning(
			"fhir_jobs",
			summary.clone(),
			format!("≥{WARN_ERROR_PCT}% errored"),
		)
	} else {
		Check::pass("fhir_jobs", summary)
	};

	check.with_detail("total", total)
		.with_detail("by_status", Value::Object(by_status))
}
