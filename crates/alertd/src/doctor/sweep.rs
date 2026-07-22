use std::{collections::HashMap, path::PathBuf, sync::Arc};

use bestool_canopy::schema::CheckSeverity;
use futures::stream::{FuturesUnordered, StreamExt};
use miette::{IntoDiagnostic, Result, miette};
use node_semver::Version;
use serde_json::{Map, Value};
use tracing::{debug, warn};

use bestool_tamanu::{config::TamanuConfig, server_info::get_or_create_server_id};

use crate::doctor::{
	check::{Check, OverallResult},
	checks::{self, CheckContext, SweepContext},
	progress::{DoctorEvent, ProgressSender},
	server_info::{self, ServerFacts},
};

/// Ceiling applied to a check that canopy's severity map doesn't mention.
///
/// A check absent from the map is one canopy doesn't yet know about (it's newer
/// than the deployment), which canopy's own contract classifies as `warn` until
/// it catches up. Matching that here keeps a local sweep's verdict in step with
/// what canopy would show.
const ABSENT_CHECK_SEVERITY: CheckSeverity = CheckSeverity::Warn;

/// The ceiling to apply to the check named `name`, honouring the absent-check
/// default ([`ABSENT_CHECK_SEVERITY`]).
pub fn severity_ceiling(severities: &HashMap<String, CheckSeverity>, name: &str) -> CheckSeverity {
	severities
		.get(name)
		.copied()
		.unwrap_or(ABSENT_CHECK_SEVERITY)
}

/// Environment variable holding a generic (non-Tamanu) `postgresql://` URL.
///
/// Consulted only when there is no Tamanu install and no
/// [`TAMANU_DATABASE_URL`] override: the sweep still gets a database to run
/// the generic postgres checks against, while Tamanu-specific checks skip.
///
/// [`TAMANU_DATABASE_URL`]: bestool_tamanu::config::DATABASE_URL_ENV
pub const GENERIC_DATABASE_URL_ENV: &str = "DATABASE_URL";

/// The [`GENERIC_DATABASE_URL_ENV`] fallback, if set to a non-empty value.
fn generic_database_url() -> Option<String> {
	std::env::var(GENERIC_DATABASE_URL_ENV)
		.ok()
		.filter(|s| !s.is_empty())
}

/// The database context a sweep runs against, when the host has one.
///
/// Usually a Tamanu deployment; with only a generic [`GENERIC_DATABASE_URL_ENV`]
/// it's a bare postgres and `is_tamanu` is false.
#[derive(Clone)]
pub struct SweepTamanu {
	pub version: Version,
	pub root: PathBuf,
	pub config: Arc<TamanuConfig>,
	pub database_url: String,
	/// `false` when this was synthesised from a database URL with no Tamanu
	/// install on the host: DB checks run, but install-dependent ones
	/// (the install metadata, local HTTP, caddy, services, kopia) skip.
	pub has_install: bool,
	/// `false` when the URL came from the generic [`GENERIC_DATABASE_URL_ENV`]
	/// fallback: it points at a postgres that isn't necessarily Tamanu's, so
	/// only the generic database checks run against it and Tamanu-specific
	/// ones (which query Tamanu tables) skip.
	pub is_tamanu: bool,
}

/// Resolve the database context for a sweep from an optionally-discovered
/// install.
///
/// * `Some(install)` → a real install: its config is loaded and `has_install`
///   is true.
/// * no install but [`TAMANU_DATABASE_URL`] set → a DB-only context synthesised
///   from that URL (`has_install` false) so DB checks still run against it.
/// * no install but [`GENERIC_DATABASE_URL_ENV`] set → a generic (non-Tamanu)
///   database context (`is_tamanu` false): the generic DB checks run, all
///   Tamanu-specific ones skip.
/// * none of those → `None`: host-level checks only.
///
/// [`TAMANU_DATABASE_URL`]: bestool_tamanu::config::DATABASE_URL_ENV
pub fn resolve_sweep_tamanu(install: Option<(Version, PathBuf)>) -> Result<Option<SweepTamanu>> {
	resolve_sweep_tamanu_from(
		install,
		bestool_tamanu::config::database_url_override(),
		generic_database_url(),
	)
}

/// [`resolve_sweep_tamanu`] with the environment reads made explicit, so the
/// resolution order is testable without mutating process-global env vars.
fn resolve_sweep_tamanu_from(
	install: Option<(Version, PathBuf)>,
	tamanu_url: Option<String>,
	generic_url: Option<String>,
) -> Result<Option<SweepTamanu>> {
	use bestool_tamanu::config::{Database, TamanuConfig, load_config};

	match install {
		Some((version, root)) => {
			let config = load_config(&root, None)?;
			let database_url = config.database_url();
			Ok(Some(SweepTamanu {
				version,
				root,
				config: Arc::new(config),
				database_url,
				has_install: true,
				is_tamanu: true,
			}))
		}
		None => {
			let (url, is_tamanu) = match (tamanu_url, generic_url) {
				(Some(url), _) => (url, true),
				(None, Some(url)) => (url, false),
				(None, None) => return Ok(None),
			};
			let db = Database::from_url(&url)?;
			Ok(Some(SweepTamanu {
				version: Version::parse("0.0.0").into_diagnostic()?,
				root: PathBuf::new(),
				config: Arc::new(TamanuConfig::from_database(db)),
				database_url: url,
				has_install: false,
				is_tamanu,
			}))
		}
	}
}

#[derive(Clone)]
pub struct SweepResult {
	pub server_id: Option<String>,
	pub results: Vec<(Check, bool)>,
	pub overall: OverallResult,
	pub payload: Value,
	/// `SELECT version()` result observed during this sweep, available so
	/// callers (e.g. the daemon plugin) can cache it across ticks instead of
	/// re-querying every minute.
	pub pg_version: Option<String>,
}

impl SweepResult {
	/// Lower each check's status to canopy's effective-severity ceiling, then
	/// re-derive the overall result and the wire `health[]` array to match.
	///
	/// This is a display-time transform for local consumers (the `doctor` CLI):
	/// the payload posted to canopy is always the raw one, since canopy is the
	/// source of truth for severities and applies the mapping itself. See
	/// [`CheckStatus::cap_to`](crate::doctor::check::CheckStatus::cap_to) for the
	/// ceiling semantics and [`severity_ceiling`] for the absent-check default.
	pub fn apply_severities(&mut self, severities: &HashMap<String, CheckSeverity>) {
		for (check, _) in &mut self.results {
			let ceiling = severity_ceiling(severities, check.name);
			check.status = check.status.clone().cap_to(ceiling);
		}
		self.overall = OverallResult::from_checks(
			&self
				.results
				.iter()
				.map(|(c, _)| c.clone())
				.collect::<Vec<_>>(),
		);
		if let Value::Object(obj) = &mut self.payload {
			obj.insert("health".into(), Value::Array(health_array(&self.results)));
		}
	}
}

pub async fn perform_sweep(
	binary_version: &str,
	tamanu: Option<SweepTamanu>,
	http_client: reqwest::Client,
	selected_names: &[String],
	skip_names: &[String],
	cached_pg_version: Option<String>,
	progress: Option<ProgressSender>,
) -> Result<SweepResult> {
	let tamanu_ctx = match &tamanu {
		Some(t) => {
			// Open a single connection up-front. Checks that need the DB share
			// it; the `db_connect` check separately measures the open latency
			// for reporting. Goes through `bestool_postgres::pool::connect_one`
			// so all DB opens in the project share one SSL fallback / auth
			// retry / app-name path.
			let db =
				match bestool_postgres::pool::connect_one(&t.database_url, "bestool-tamanu-doctor")
					.await
				{
					Ok(client) => Some(Arc::new(client)),
					Err(err) => {
						warn!(%err, "doctor could not open Tamanu DB; DB-dependent checks will skip");
						None
					}
				};

			// A generic (non-Tamanu) database has no Tamanu tables to inspect,
			// so don't probe it for kind or version; the value is unused since
			// every Tamanu-dependent check skips.
			let kind = if t.is_tamanu {
				let kind = bestool_tamanu::detect_kind(&t.config, db.as_deref()).await;
				debug!(?kind, "detected Tamanu server kind for doctor sweep");
				kind
			} else {
				bestool_tamanu::ApiServerKind::Central
			};

			// With a real install, the version is the env-file/install version.
			// Without one (a `TAMANU_DATABASE_URL`-only host), fall back to the
			// version Tamanu last recorded in its own DB (`currentVersion`), so
			// version-aware checks can still run against it.
			let tamanu_version = match (t.has_install, db.as_deref()) {
				(false, Some(client)) if t.is_tamanu => {
					bestool_tamanu::versions::current_version(client)
						.await
						.unwrap_or_else(|| t.version.clone())
				}
				_ => t.version.clone(),
			};

			Some(CheckContext {
				tamanu_version,
				tamanu_root: t.root.clone(),
				config: t.config.clone(),
				kind,
				database_url: t.database_url.clone(),
				db,
				http_client: http_client.clone(),
				has_install: t.has_install,
				is_tamanu: t.is_tamanu,
			})
		}
		None => None,
	};
	let db = tamanu_ctx.as_ref().and_then(|c| c.db.clone());
	// The version resolved above (install version, or the DB's `currentVersion`
	// for a database-only host), kept for the wire payload after `tamanu_ctx` is
	// moved into the check context below. The server kind and (when there's a
	// real install) its root go into the top-level status facts too.
	let resolved_version = tamanu_ctx
		.as_ref()
		.filter(|c| c.is_tamanu)
		.map(|c| c.tamanu_version.clone());
	let tamanu_server_kind = tamanu_ctx
		.as_ref()
		.filter(|c| c.is_tamanu)
		.map(|c| match c.kind {
			bestool_tamanu::ApiServerKind::Central => "central",
			bestool_tamanu::ApiServerKind::Facility => "facility",
		});
	let tamanu_root = tamanu
		.as_ref()
		.filter(|t| t.has_install)
		.map(|t| t.root.display().to_string());

	let check_ctx = SweepContext {
		tamanu: tamanu_ctx,
		http_client,
	};

	let registry = checks::all();
	let known: Vec<&str> = registry.iter().map(|e| e.name).collect();
	if let Some(unknown) = selected_names.iter().find(|n| !known.contains(&n.as_str())) {
		return Err(miette!(
			"unknown check name `{unknown}`; known checks: {}",
			known.join(", ")
		));
	}
	if let Some(unknown) = skip_names.iter().find(|n| !known.contains(&n.as_str())) {
		return Err(miette!(
			"unknown check name `{unknown}` in --skip; known checks: {}",
			known.join(", ")
		));
	}

	let selected: Vec<(usize, &checks::CheckEntry)> = registry
		.iter()
		.enumerate()
		.filter(|(_, e)| selected_names.is_empty() || selected_names.iter().any(|n| n == e.name))
		.filter(|(_, e)| !skip_names.iter().any(|n| n == e.name))
		.collect();

	// Run all selected checks concurrently. Results are collated by registry
	// index before returning, so callers see a stable order regardless of
	// completion order. A progress channel can observe results as they land.
	let mut pending = FuturesUnordered::new();
	for (idx, entry) in &selected {
		let ctx = check_ctx.clone();
		let on_wire = entry.on_wire;
		let idx = *idx;
		let fut = (entry.run)(ctx);
		pending.push(async move {
			let result = fut.await;
			(idx, on_wire, result)
		});
	}

	let mut completed: Vec<(usize, Check, bool)> = Vec::with_capacity(selected.len());
	while let Some((idx, on_wire, check)) = pending.next().await {
		if let Some(tx) = progress.as_ref() {
			let _ = tx.send(DoctorEvent::Completed(check.clone()));
		}
		completed.push((idx, check, on_wire));
	}
	completed.sort_by_key(|(idx, _, _)| *idx);
	let results: Vec<(Check, bool)> = completed.into_iter().map(|(_, c, w)| (c, w)).collect();

	// Resolve via the file path first so a doctor sweep can still report to
	// canopy when the DB is down — that's exactly the moment canopy most
	// needs to hear from us.
	let server_id = match get_or_create_server_id().await {
		Ok(id) => Some(id),
		Err(err) => {
			warn!("could not resolve metaServerId: {err}");
			None
		}
	};

	let mut facts = collect_server_facts(
		tamanu.as_ref().map(|t| t.config.as_ref()),
		db.as_deref(),
		cached_pg_version,
		tamanu.as_ref().is_none_or(|t| t.is_tamanu),
	)
	.await;
	facts.tamanu_root = tamanu_root;
	facts.tamanu_server_kind = tamanu_server_kind;
	let pg_version = facts.pg_version.clone();
	// `binary_version` is the running binary's (bestool's) version, threaded in
	// by the caller. Evaluating `env!("CARGO_PKG_VERSION")` here would resolve
	// to this library's version instead, which is the wrong answer for the wire
	// payload. On hosts with no Tamanu (including generic-database-only hosts),
	// `tamanuVersion` is omitted from the payload entirely. A Tamanu
	// database-only host reports the version resolved from its DB.
	let tamanu_version = resolved_version.map(|v| v.to_string());
	let info = server_info::gather(binary_version, tamanu_version, facts).await;
	let info_value = serde_json::to_value(&info).into_diagnostic()?;

	let overall =
		OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
	let payload = build_payload(&info_value, &results);

	Ok(SweepResult {
		server_id,
		results,
		overall,
		payload,
		pg_version,
	})
}

async fn collect_server_facts(
	config: Option<&TamanuConfig>,
	db: Option<&tokio_postgres::Client>,
	cached_pg_version: Option<String>,
	is_tamanu: bool,
) -> ServerFacts {
	let mut facts = ServerFacts {
		canonical_url: config
			.and_then(|c| c.canonical_url())
			.map(|u| u.to_string()),
		timezone: config
			.and_then(|c| c.primary_time_zone())
			.map(|s| s.to_string()),
		pg_version: cached_pg_version,
		..Default::default()
	};

	let Some(client) = db else {
		return facts;
	};

	if facts.pg_version.is_none() {
		match client.query_one("SELECT version()", &[]).await {
			Ok(row) => match row.try_get::<_, String>(0) {
				Ok(v) => facts.pg_version = Some(v),
				Err(err) => warn!("decoding pg_version: {err}"),
			},
			Err(err) => warn!("SELECT version() failed: {err}"),
		}
	}

	// `local_system_facts` is a Tamanu table; a generic database has no
	// sync tick to read.
	if is_tamanu {
		match client
			.query_opt(
				"SELECT value FROM local_system_facts WHERE key = 'currentSyncTick'",
				&[],
			)
			.await
		{
			Ok(Some(row)) => match row.try_get::<_, String>(0) {
				Ok(tick) => facts.current_sync_tick = Some(tick),
				Err(err) => warn!("decoding currentSyncTick: {err}"),
			},
			Ok(None) => {}
			Err(err) => warn!("querying currentSyncTick: {err}"),
		}
	}

	facts
}

pub fn overall_from_payload(payload: &Value) -> OverallResult {
	let results = || {
		payload
			.get("health")
			.and_then(Value::as_array)
			.into_iter()
			.flatten()
			.filter_map(|c| c.get("result").and_then(Value::as_str))
	};
	if results().any(|r| r == "failed") {
		OverallResult::Failing
	} else if results().any(|r| r == "warning" || r == "broken") {
		OverallResult::Degraded
	} else {
		OverallResult::Healthy
	}
}

fn build_payload(info: &Value, results: &[(Check, bool)]) -> Value {
	let mut payload: Map<String, Value> = match info {
		Value::Object(o) => o.clone(),
		_ => Map::new(),
	};

	// Lift any `payload_extras` from individual checks into the top-level
	// payload (alongside server facts like `osTimezone`). Lets a check carry
	// bulky context-data that belongs with server facts rather than crowding
	// its diagnostic entry in `health[]`.
	for (check, _) in results {
		for (k, v) in &check.payload_extras {
			payload.insert(k.clone(), v.clone());
		}
	}

	payload.insert("health".into(), Value::Array(health_array(results)));

	Value::Object(payload)
}

/// The `health[]` wire array: one entry per on-wire check, in the given order.
fn health_array(results: &[(Check, bool)]) -> Vec<Value> {
	results
		.iter()
		.filter(|(_, on_wire)| *on_wire)
		.map(|(c, _)| c.to_wire())
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn pass(name: &'static str) -> (Check, bool) {
		(Check::pass(name, "ok"), true)
	}
	fn warn(name: &'static str) -> (Check, bool) {
		(Check::warning(name, "deg", "reason"), true)
	}
	fn fail(name: &'static str) -> (Check, bool) {
		(Check::fail(name, "bad", "reason"), true)
	}
	fn skip(name: &'static str) -> (Check, bool) {
		(Check::skip(name, "not run", "reason"), true)
	}

	#[tokio::test]
	async fn sweep_without_tamanu_skips_tamanu_checks_and_runs_host_checks() {
		// Restrict to a deterministic on-wire subset: a tamanu-dependent check
		// plus one host check with no external dependencies.
		let sweep = perform_sweep(
			"0.0.0-test",
			None,
			reqwest::Client::new(),
			&["tamanu_http".into(), "memory".into()],
			&[],
			None,
			None,
		)
		.await
		.unwrap();

		let result_of = |name: &str| {
			sweep.payload["health"]
				.as_array()
				.unwrap()
				.iter()
				.find(|c| c["check"] == name)
				.unwrap_or_else(|| panic!("{name} missing from health[]"))["result"]
				.clone()
		};
		assert_eq!(result_of("tamanu_http"), "skipped");
		assert_ne!(result_of("memory"), "skipped");
		// No Tamanu means no tamanuVersion on the wire at all.
		assert!(sweep.payload.get("tamanuVersion").is_none());
	}

	#[test]
	fn payload_all_pass() {
		let results = vec![pass("a"), pass("b")];
		let payload = build_payload(&Value::Object(Default::default()), &results);
		assert!(payload.get("healthy").is_none());
		assert_eq!(payload["health"].as_array().unwrap().len(), 2);
		assert_eq!(payload["health"][0]["result"], "passed");
	}

	#[test]
	fn payload_per_check_results() {
		let results = vec![pass("a"), warn("b"), fail("c")];
		let payload = build_payload(&Value::Object(Default::default()), &results);
		assert_eq!(payload["health"][0]["result"], "passed");
		assert_eq!(payload["health"][1]["result"], "warning");
		assert_eq!(payload["health"][2]["result"], "failed");
	}

	#[test]
	fn overall_from_payload_tiers_on_results() {
		let mk = |results: &[&str]| {
			serde_json::json!({
				"health": results.iter().map(|r| serde_json::json!({"check": "x", "result": r})).collect::<Vec<_>>(),
			})
		};
		assert_eq!(
			overall_from_payload(&mk(&["passed", "skipped"])),
			OverallResult::Healthy
		);
		assert_eq!(
			overall_from_payload(&mk(&["passed", "warning"])),
			OverallResult::Degraded
		);
		assert_eq!(
			overall_from_payload(&mk(&["passed", "broken"])),
			OverallResult::Degraded
		);
		assert_eq!(
			overall_from_payload(&mk(&["warning", "failed"])),
			OverallResult::Failing
		);
	}

	#[test]
	fn payload_lifts_payload_extras_into_top_level() {
		// `payload_extras` is for data a check wants alongside server facts
		// (osTimezone etc), not in its per-check entry. The tamanu_service
		// check uses it for raw service inventory.
		let mut info = serde_json::Map::new();
		info.insert("osTimezone".into(), "Pacific/Auckland".into());
		let info_value = Value::Object(info);

		let check = Check::pass("svc", "ok")
			.with_detail("supervisor", "systemd")
			.with_payload_extra(
				"services",
				serde_json::json!({"supervisor": "systemd", "expectations": []}),
			);
		let results = vec![(check, true)];
		let payload = build_payload(&info_value, &results);

		assert_eq!(payload["osTimezone"], "Pacific/Auckland");
		// Lifted into the top level, alongside osTimezone.
		assert_eq!(payload["services"]["supervisor"], "systemd");
		// And NOT duplicated into the per-check entry.
		assert!(payload["health"][0].get("services").is_none());
		// But the lean per-check detail (supervisor label) is still on the
		// `health[]` entry.
		assert_eq!(payload["health"][0]["supervisor"], "systemd");
	}

	#[test]
	fn off_wire_checks_skipped_in_health_array() {
		let results = vec![
			(Check::pass("on", "ok"), true),
			(Check::pass("off", "ok"), false),
		];
		let payload = build_payload(&Value::Object(Default::default()), &results);
		let names: Vec<&str> = payload["health"]
			.as_array()
			.unwrap()
			.iter()
			.map(|v| v["check"].as_str().unwrap())
			.collect();
		assert_eq!(names, vec!["on"]);
	}

	#[test]
	fn apply_severities_caps_results_overall_and_wire() {
		let results = vec![pass("keep_pass"), fail("noisy"), fail("real")];
		let payload = build_payload(&Value::Object(Default::default()), &results);
		let mut sweep = SweepResult {
			server_id: None,
			results,
			overall: OverallResult::Failing,
			payload,
			pg_version: None,
		};

		let mut severities = HashMap::new();
		severities.insert("noisy".to_string(), CheckSeverity::Skip);
		severities.insert("real".to_string(), CheckSeverity::Fail);
		// `keep_pass` is absent from the map: it defaults to warn, but a pass is
		// never raised, so it stays passing.
		sweep.apply_severities(&severities);

		let status_of = |name: &str| {
			sweep
				.results
				.iter()
				.find(|(c, _)| c.name == name)
				.map(|(c, _)| c.status.wire_result())
				.unwrap()
		};
		assert_eq!(status_of("keep_pass"), "passed");
		assert_eq!(status_of("noisy"), "skipped");
		assert_eq!(status_of("real"), "failed");

		// Overall is re-derived from the capped statuses: the only fatal check
		// left is `real`, so we're still failing; silence `real` too and it drops.
		assert_eq!(sweep.overall, OverallResult::Failing);

		// The wire health array tracks the capped statuses.
		let wire_result = |name: &str| {
			sweep.payload["health"]
				.as_array()
				.unwrap()
				.iter()
				.find(|c| c["check"] == name)
				.unwrap()["result"]
				.clone()
		};
		assert_eq!(wire_result("noisy"), "skipped");
		assert_eq!(wire_result("real"), "failed");
	}

	#[test]
	fn apply_severities_absent_check_defaults_to_warn() {
		// A check canopy hasn't heard of yet is capped at warn, so a computed
		// failure is shown as a warning rather than promoted or left fatal.
		let results = vec![fail("brand_new")];
		let payload = build_payload(&Value::Object(Default::default()), &results);
		let mut sweep = SweepResult {
			server_id: None,
			results,
			overall: OverallResult::Failing,
			payload,
			pg_version: None,
		};
		sweep.apply_severities(&HashMap::new());
		assert_eq!(sweep.results[0].0.status.wire_result(), "warning");
		assert_eq!(sweep.overall, OverallResult::Degraded);
	}

	#[test]
	fn resolve_prefers_tamanu_url_over_generic() {
		let resolved = resolve_sweep_tamanu_from(
			None,
			Some("postgresql://u@localhost/tamanu".into()),
			Some("postgresql://u@localhost/other".into()),
		)
		.unwrap()
		.unwrap();
		assert_eq!(resolved.database_url, "postgresql://u@localhost/tamanu");
		assert!(resolved.is_tamanu);
		assert!(!resolved.has_install);
	}

	#[test]
	fn resolve_falls_back_to_generic_database_url() {
		let resolved =
			resolve_sweep_tamanu_from(None, None, Some("postgresql://u@localhost/other".into()))
				.unwrap()
				.unwrap();
		assert_eq!(resolved.database_url, "postgresql://u@localhost/other");
		assert!(!resolved.is_tamanu);
		assert!(!resolved.has_install);
	}

	#[test]
	fn resolve_without_any_url_is_none() {
		assert!(
			resolve_sweep_tamanu_from(None, None, None)
				.unwrap()
				.is_none()
		);
	}

	#[test]
	fn payload_skip_result_on_wire() {
		// The whole point of distinguishing Skip from Fail/Warning is that
		// "we don't know" shouldn't fire alerts downstream of the wire format.
		let results = vec![pass("a"), skip("b")];
		let payload = build_payload(&Value::Object(Default::default()), &results);
		assert_eq!(payload["health"][1]["result"], "skipped");
	}
}
