//! Check that every running tamanu container is on the version the
//! deployment is configured for. A mismatch means the env file has been
//! bumped (or rolled back) but at least one container is still on the
//! previous tag — a half-rolled-out upgrade, blue/green swap that didn't
//! complete, etc. The user-visible symptom for those is "everything looks
//! OK in `tamanu status`" but the service is actually serving stale code.

use serde_json::{Value, json};

use bestool_tamanu::{
	services::{Supervisor, expected, parse_systemd_unit, systemd_patient_portal_instanced},
	versions,
};

use super::CheckContext;
use crate::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let supervisor = if cfg!(target_os = "linux") {
		Supervisor::Systemd
	} else if cfg!(target_os = "windows") {
		Supervisor::Pm2
	} else {
		return Check::skip(
			"version_drift",
			"version drift check skipped on this platform",
			"only Linux/systemd and Windows/pm2 deployments carry version metadata",
		);
	};

	if matches!(supervisor, Supervisor::Pm2) {
		// pm2 deployments share an install root; every process necessarily
		// runs the version `find_tamanu` returned. There's no drift to
		// detect at the supervisor level.
		return Check::pass(
			"version_drift",
			format!(
				"pm2 install at v{}; no per-process drift",
				ctx.tamanu_version
			),
		)
		.with_detail("supervisor", "pm2")
		.with_detail("install_version", ctx.tamanu_version.to_string());
	}

	let expected_versions = versions::expected_for_supervisor(supervisor, &ctx.tamanu_version);
	let running = versions::running_versions_linux().await;

	// Only look at units that show up in our expectations registry. Hand-
	// started or orphaned containers aren't drift; they're outside the
	// expected set.
	let patient_portal_enabled = match ctx.db.as_deref() {
		Some(client) => bestool_tamanu::server_info::query_patient_portal_enabled(client).await,
		None => None,
	};
	let patient_portal_instanced =
		matches!(supervisor, Supervisor::Systemd) && systemd_patient_portal_instanced().await;
	let expectations = expected(
		supervisor,
		ctx.kind,
		&ctx.config,
		patient_portal_enabled,
		patient_portal_instanced,
	);

	let mut rows: Vec<Value> = Vec::new();
	let mut drifted: Vec<String> = Vec::new();
	let mut total_running = 0usize;

	for (unit, actual) in &running {
		let Some((base, _instance)) = parse_systemd_unit(unit) else {
			continue;
		};
		let Some(exp) = expectations.iter().find(|e| e.name == base) else {
			// Container running for a unit we don't expect (e.g. legacy or
			// hand-managed). Not our concern.
			continue;
		};
		total_running += 1;
		let exp_v = expected_versions.for_service(exp.name);
		let status = versions::classify(Some(actual.as_str()), exp_v);
		rows.push(json!({
			"unit": unit,
			"expected": exp_v,
			"actual": actual,
			"status": match status {
				versions::VersionStatus::Match => "match",
				versions::VersionStatus::Mismatch => "mismatch",
				versions::VersionStatus::Unknown => "unknown",
			},
		}));
		if status.is_mismatch() {
			drifted.push(format!(
				"{unit}: expected {} but running {actual}",
				exp_v.unwrap_or("?"),
			));
		}
	}

	let expected_summary = json!({
		"tamanu": expected_versions.tamanu,
		"frontend": expected_versions.frontend,
	});

	if drifted.is_empty() {
		let summary = if total_running == 0 {
			"no running tamanu containers detected".to_string()
		} else {
			let tag = expected_versions.tamanu.as_deref().unwrap_or("(unknown)");
			format!("{total_running} container(s) on expected version {tag}")
		};
		Check::pass("version_drift", summary)
			.with_detail("expected", expected_summary)
			.with_detail("instances", Value::Array(rows))
	} else {
		let summary = format!("{} container(s) on a stale version", drifted.len());
		Check::fail("version_drift", summary, drifted.join("; "))
			.with_detail("expected", expected_summary)
			.with_detail("instances", Value::Array(rows))
	}
}
