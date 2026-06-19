//! Check that every running tamanu container is on the version the
//! deployment is configured for. A mismatch means the env file has been
//! bumped (or rolled back) but at least one container is still on the
//! previous tag — a half-rolled-out upgrade, blue/green swap that didn't
//! complete, etc. The user-visible symptom for those is "everything looks
//! OK in `tamanu status`" but the service is actually serving stale code.

use std::collections::HashMap;

use serde_json::{Value, json};

use bestool_tamanu::{
	services::{
		Expectation, Supervisor, expected, parse_systemd_unit, systemd_patient_portal_instanced,
	},
	versions::{self, ExpectedVersions},
};

use super::CheckContext;
use crate::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	// The comparison baseline is the install's env-file version when present,
	// else the DB's recorded `currentVersion`. If neither resolved, the version
	// is the 0.0.0 sentinel and there's nothing to compare against — skip rather
	// than flag every running container as drifted.
	if ctx.tamanu_version.major == 0
		&& ctx.tamanu_version.minor == 0
		&& ctx.tamanu_version.patch == 0
	{
		return Check::skip(
			"version_drift",
			"Tamanu version unknown",
			"no install on disk and the database has no recorded currentVersion to compare against",
		);
	}

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

	let running = match versions::running_versions_linux().await {
		Ok(map) => map,
		// We couldn't read what's running, so we can't judge drift. Bail before
		// the DB round-trip below — there's nothing to compare against.
		Err(reason) => return unreadable_check(&reason),
	};
	let expected_versions = versions::expected_for_supervisor(supervisor, &ctx.tamanu_version);

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
		Some(ctx.config.as_ref()),
		patient_portal_enabled,
		patient_portal_instanced,
	);

	evaluate_drift(&running, &expected_versions, &expectations)
}

/// Warning result for when `podman ps` couldn't be read at all. We can't judge
/// drift, and must not report a pass — that would dress up a blind check as a
/// healthy one. Most often this is alertd lacking access to the root-owned
/// containers (see the podman-socket / privilege notes).
fn unreadable_check(reason: &str) -> Check {
	Check::warning(
		"version_drift",
		"could not read running container versions",
		format!("`podman ps` failed, so version drift can't be checked: {reason}"),
	)
	.with_detail("supervisor", "systemd")
}

/// Compare each running container's image tag against the version the
/// deployment is configured for, given an already-read `running` map. An empty
/// map is a genuine "nothing running" pass — distinct from the unreadable case
/// handled by [`unreadable_check`].
fn evaluate_drift(
	running: &HashMap<String, String>,
	expected_versions: &ExpectedVersions,
	expectations: &[Expectation],
) -> Check {
	let mut rows: Vec<Value> = Vec::new();
	let mut drifted: Vec<String> = Vec::new();
	let mut total_running = 0usize;

	for (unit, actual) in running {
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::check::CheckStatus;
	use bestool_tamanu::services::{ExpectedState, Instances};

	fn exp(name: &'static str, instances: Instances) -> Expectation {
		Expectation {
			name,
			instances,
			state: ExpectedState::Up,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
		}
	}

	fn ev(tamanu: &str, frontend: Option<&str>) -> ExpectedVersions {
		ExpectedVersions {
			tamanu: Some(tamanu.into()),
			frontend: frontend.map(Into::into),
		}
	}

	#[test]
	fn unreadable_is_warning_not_pass() {
		// The regression that motivated this: a blind check must NOT look healthy.
		let check = unreadable_check("podman not found on PATH");
		assert!(matches!(check.status, CheckStatus::Warning(_)), "{check:?}");
	}

	#[test]
	fn empty_running_is_pass() {
		// podman answered with nothing running — genuinely fine, distinct from blind.
		let exps = [exp("tamanu-central-api", Instances::NumericAtLeast(2))];
		let check = evaluate_drift(&HashMap::new(), &ev("v2.54.7", None), &exps);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn matching_versions_pass() {
		let exps = [exp("tamanu-central-api", Instances::NumericAtLeast(2))];
		let running = HashMap::from([
			(
				"tamanu-central-api@1.service".to_string(),
				"v2.54.7".to_string(),
			),
			(
				"tamanu-central-api@2.service".to_string(),
				"v2.54.7".to_string(),
			),
		]);
		let check = evaluate_drift(&running, &ev("v2.54.7", None), &exps);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn drifted_frontend_fails_naming_the_unit() {
		// env wants frontend v2.54.12 but the container is still on v2.54.7 —
		// exactly the case `tamanu status` couldn't see when run unprivileged.
		let exps = [
			exp("tamanu-frontend", Instances::Named(&["a", "b"])),
			exp("tamanu-central-api", Instances::NumericAtLeast(2)),
		];
		let running = HashMap::from([(
			"tamanu-frontend@a.service".to_string(),
			"v2.54.7".to_string(),
		)]);
		let check = evaluate_drift(&running, &ev("v2.54.7", Some("v2.54.12")), &exps);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains("tamanu-frontend@a"), "{reason}")
			}
			other => panic!("expected fail, got {other:?}"),
		}
	}

	#[test]
	fn unexpected_unit_is_not_drift() {
		let exps = [exp("tamanu-central-api", Instances::NumericAtLeast(2))];
		let running =
			HashMap::from([("tamanu-orphan@1.service".to_string(), "v1.0.0".to_string())]);
		let check = evaluate_drift(&running, &ev("v2.54.7", None), &exps);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}
}
