//! FHIR job queue depth.
//!
//! Tamanu's `fhir.jobs` table is both queue and audit log: rows in status
//! `Errored` outlive the work they describe, as the record of a past failure,
//! so the count of those is not a signal of current health. What we care
//! about is the *active queue* — rows that workers haven't yet drained:
//! `Queued`, `Grabbed`, `Started`.

use jiff::Timestamp;
use miette::IntoDiagnostic as _;
use serde_json::{Map, Value};

use bestool_tamanu::{
	ApiServerKind,
	config::TamanuConfig,
	pm2,
	services::{ExpectedState, Supervisor, expected},
	systemd,
};
use tracing::{info, warn};

use super::util::humanise_age;
use super::{CheckContext, SweepContext, query_error_check};
use crate::doctor::Stat;
use crate::doctor::check::Check;
use crate::doctor::heal::HealOutcome;

const WARN_DEPTH: i64 = 200;
const FAIL_DEPTH: i64 = 2_000;
const WARN_OLDEST_SECS: i64 = 10 * 60; // 10m
const FAIL_OLDEST_SECS: i64 = 60 * 60; // 1h

pub async fn run(ctx: CheckContext) -> Check {
	if ctx.config.is_facility() {
		return Check::skip(
			"fhir_jobs",
			"not applicable on facility server",
			"central-only check",
		);
	}

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
				return Check::skip("fhir_jobs", "fhir.jobs table not present", "table absent");
			}
			return query_error_check("fhir_jobs", &err);
		}
	};

	let active: i64 = row.try_get("active_depth").unwrap_or(0);
	let oldest_active: Option<Timestamp> = row.try_get("oldest_active").ok();

	let (by_status_value, by_status_counts) = match client
		.query(
			"SELECT status, count(*)::bigint AS n FROM fhir.jobs GROUP BY status",
			&[],
		)
		.await
	{
		Ok(rows) => {
			let mut by: Map<String, Value> = Map::new();
			let mut counts: Vec<(String, i64)> = Vec::new();
			for row in rows {
				let status: String = row.try_get("status").unwrap_or_default();
				let n: i64 = row.try_get("n").unwrap_or(0);
				by.insert(status.clone(), Value::from(n));
				counts.push((status, n));
			}
			(Value::Object(by), counts)
		}
		Err(_) => (Value::Object(Map::new()), Vec::new()),
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
		.with_detail("by_status", by_status_value)
		.with_stat(
			Stat::gauge("active_depth", active as f64).help("Active FHIR jobs (not Errored)"),
		)
		.with_stats(by_status_counts.into_iter().map(|(status, n)| {
			Stat::gauge("jobs", n as f64)
				.label("status", status)
				.help("FHIR jobs by status")
		}));
	if let Some(ts) = oldest_active {
		check = check
			.with_detail("oldest_active", ts.to_string())
			.with_detail("oldest_active_age_secs", oldest_age_secs)
			.with_stat(
				Stat::gauge("oldest_active_age_seconds", oldest_age_secs as f64)
					.help("Age of the oldest active FHIR job"),
			);
	}
	check
}

/// Restart the FHIR workers to recover a stalled jobs backlog.
///
/// Central-only, and only when the worker is enabled in configuration — a
/// worker disabled in config is meant to be down, so restarting it is wrong
/// (the FHIR config check grades that instead). A restart does not drain a
/// deep backlog before the next sweep, so the check keeps failing until the
/// queue clears; the daemon caps this heal at one attempt an hour (set on the
/// check's registry entry) so a slowly-draining queue is not repeatedly kicked.
///
/// spec: CHK-FHJ#self-healing
pub async fn heal(ctx: SweepContext) -> HealOutcome {
	let Some(tamanu) = ctx.tamanu.as_ref() else {
		return HealOutcome::Deferred;
	};
	if !matches!(tamanu.kind, ApiServerKind::Central) {
		return HealOutcome::Deferred;
	}
	if !tamanu.config.fhir_worker_enabled() {
		return HealOutcome::Deferred;
	}
	let Some(supervisor) = Supervisor::current() else {
		return HealOutcome::Deferred;
	};

	let targets = fhir_worker_targets(supervisor, tamanu.kind, tamanu.config.as_ref());
	if targets.is_empty() {
		return HealOutcome::Deferred;
	}

	let result = match supervisor {
		Supervisor::Systemd => systemd::restart_all(&targets).await,
		// pm2's restart is a blocking stop→pause→start, so keep it off the async
		// worker thread.
		Supervisor::Pm2 => tokio::task::spawn_blocking(move || pm2::restart_targets(&targets))
			.await
			.into_diagnostic()
			.and_then(|inner| inner),
	};

	match result {
		Ok(()) => {
			info!("restarted the FHIR workers to recover a stalled jobs backlog");
			HealOutcome::Healed
		}
		Err(err) => {
			warn!(%err, "fhir_jobs heal: restarting the FHIR workers failed");
			HealOutcome::Failed
		}
	}
}

/// The supervisor targets for the FHIR worker services expected Up — systemd
/// unit names, or pm2 process names — derived from the service expectations so
/// the naming stays in step with the rest of the lifecycle tooling.
fn fhir_worker_targets(
	supervisor: Supervisor,
	kind: ApiServerKind,
	config: &TamanuConfig,
) -> Vec<String> {
	expected(supervisor, kind, Some(config), None, false)
		.into_iter()
		.filter(|e| e.name.contains("fhir") && matches!(e.state, ExpectedState::Up))
		.flat_map(|e| match supervisor {
			Supervisor::Systemd => e.instances.required_systemd_units(e.name),
			Supervisor::Pm2 => vec![e.name.to_string()],
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn central_config(worker_enabled: bool) -> TamanuConfig {
		serde_json::from_value(serde_json::json!({
			"db": { "name": "tamanu-central", "username": "u", "password": "p" },
			"integrations": { "fhir": { "worker": { "enabled": worker_enabled } } },
		}))
		.expect("central test config should parse")
	}

	#[test]
	fn systemd_targets_are_the_two_central_fhir_units() {
		let cfg = central_config(true);
		let mut targets = fhir_worker_targets(Supervisor::Systemd, ApiServerKind::Central, &cfg);
		targets.sort();
		assert_eq!(
			targets,
			vec![
				"tamanu-central-fhir-refresh.service".to_string(),
				"tamanu-central-fhir-resolve.service".to_string(),
			]
		);
	}

	#[test]
	fn pm2_targets_are_the_process_names() {
		let cfg = central_config(true);
		let mut targets = fhir_worker_targets(Supervisor::Pm2, ApiServerKind::Central, &cfg);
		targets.sort();
		assert_eq!(
			targets,
			vec![
				"tamanu-fhir-refresh".to_string(),
				"tamanu-fhir-resolve".to_string(),
			]
		);
	}

	#[test]
	fn no_targets_when_worker_disabled() {
		// A disabled worker is expected Down, so there is nothing to restart.
		let cfg = central_config(false);
		assert!(
			fhir_worker_targets(Supervisor::Systemd, ApiServerKind::Central, &cfg).is_empty(),
			"a config-disabled worker yields no restart targets"
		);
	}
}
