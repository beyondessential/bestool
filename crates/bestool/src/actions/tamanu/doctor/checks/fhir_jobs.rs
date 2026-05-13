//! FHIR job queue depth.
//!
//! Tamanu's `fhir.jobs` table is both queue and audit log: rows in status
//! `Errored` intentionally stick around forever (they record past failures),
//! so the count of those is not a signal of current health. What we care
//! about is the *active queue* — rows that workers haven't yet drained:
//! `Queued`, `Grabbed`, `Started`.

use jiff::Timestamp;
use serde_json::{Map, Value};

use super::{CheckContext, fmt_db_error};
use crate::actions::tamanu::doctor::check::Check;

const WARN_DEPTH: i64 = 200;
const FAIL_DEPTH: i64 = 2_000;
const WARN_OLDEST_SECS: i64 = 10 * 60; // 10m
const FAIL_OLDEST_SECS: i64 = 60 * 60; // 1h

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::fail("fhir_jobs", "no DB connection", "db_connect failed");
	};

	let agg_query = r#"
		SELECT
			count(*) FILTER (WHERE status <> 'Errored')::bigint AS active_depth,
			min(created_at) FILTER (WHERE status <> 'Errored') AS oldest_active
		FROM fhir.jobs
	"#;

	let row = match client.query_one(agg_query, &[]).await {
		Ok(r) => r,
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& (db.code() == &tokio_postgres::error::SqlState::UNDEFINED_TABLE
					|| db.code() == &tokio_postgres::error::SqlState::INVALID_SCHEMA_NAME)
			{
				return Check::pass("fhir_jobs", "fhir.jobs table not present")
					.with_detail("skipped", true);
			}
			return Check::fail("fhir_jobs", "query failed", fmt_db_error(&err));
		}
	};

	let active: i64 = row.try_get("active_depth").unwrap_or(0);
	let oldest_active: Option<Timestamp> = row.try_get("oldest_active").ok();

	let by_status_value = match client
		.query(
			"SELECT status, count(*)::bigint AS n FROM fhir.jobs GROUP BY status",
			&[],
		)
		.await
	{
		Ok(rows) => {
			let mut by: Map<String, Value> = Map::new();
			for row in rows {
				let status: String = row.try_get("status").unwrap_or_default();
				let n: i64 = row.try_get("n").unwrap_or(0);
				by.insert(status, Value::from(n));
			}
			Value::Object(by)
		}
		Err(_) => Value::Object(Map::new()),
	};

	let oldest_age_secs = oldest_active
		.map(|ts| (Timestamp::now() - ts).get_seconds())
		.unwrap_or(0);

	let summary = if active == 0 {
		"queue empty".to_string()
	} else {
		let age = humanise_age(oldest_age_secs);
		format!("{active} active (oldest {age})")
	};

	let check = if active >= FAIL_DEPTH || oldest_age_secs >= FAIL_OLDEST_SECS {
		let reason = if active >= FAIL_DEPTH {
			format!("backlog ≥{FAIL_DEPTH}")
		} else {
			format!("oldest job older than {}", humanise_age(FAIL_OLDEST_SECS))
		};
		Check::fail("fhir_jobs", summary, reason)
	} else if active >= WARN_DEPTH || oldest_age_secs >= WARN_OLDEST_SECS {
		let reason = if active >= WARN_DEPTH {
			format!("backlog ≥{WARN_DEPTH}")
		} else {
			format!("oldest job older than {}", humanise_age(WARN_OLDEST_SECS))
		};
		Check::warning("fhir_jobs", summary, reason)
	} else {
		Check::pass("fhir_jobs", summary)
	};

	let mut check = check
		.with_detail("active_depth", active)
		.with_detail("by_status", by_status_value);
	if let Some(ts) = oldest_active {
		check = check
			.with_detail("oldest_active", ts.to_string())
			.with_detail("oldest_active_age_secs", oldest_age_secs);
	}
	check
}

fn humanise_age(secs: i64) -> String {
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
