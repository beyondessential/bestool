//! Check that every running tamanu service is on the version the deployment is
//! configured for. A mismatch means the env file has been bumped (or rolled
//! back) but at least one service is still on the previous tag — a half-rolled-
//! out upgrade, blue/green swap that didn't complete, etc. The user-visible
//! symptom for those is "everything looks OK in `tamanu status`" but the service
//! is actually serving stale code.

use serde_json::{Value, json};

use bestool_tamanu::{
	services::{Expectation, Supervisor, expected, systemd_patient_portal_instanced},
	versions::{self, ExpectedVersions},
};

use super::TamanuCx;
use crate::Stat;
use crate::check::Check;
use crate::runtime::{Duty, ServiceId, facts_for};

/// One service as the drift grading needs it: what it runs, and what names it.
struct Running {
	duty: Duty,
	id: ServiceId,
	version: String,
}

pub async fn run(ctx: TamanuCx) -> Check {
	// The expected service set follows from the role the deployment plays. A
	// Tamanu context is only built for a Tamanu subject, so there is always one.
	let kind = ctx.server_kind();

	// The comparison baseline is the install's env-file version when present,
	// else the DB's recorded `currentVersion`. If neither resolved, the version
	// is the 0.0.0 sentinel and there's nothing to compare against — skip rather
	// than flag every running service as drifted.
	if ctx.version.major == 0 && ctx.version.minor == 0 && ctx.version.patch == 0 {
		return Check::skip(
			"version_drift",
			"Tamanu version unknown",
			"no install on disk and the database has no recorded currentVersion to compare against",
		);
	}

	// Where the expected versions are written differs by deployment shape — an
	// env file beside a container deployment, the install root itself under a
	// process supervisor — so the shape is still read from the platform. What
	// the platform no longer decides is how a service is matched to its
	// expectation: that goes by duty, the same on either.
	let Some(supervisor) = Supervisor::current() else {
		return Check::skip(
			"version_drift",
			"version drift check skipped on this platform",
			"only Linux/systemd and Windows/pm2 deployments carry version metadata",
		);
	};

	let services = match ctx.runtime.services().await {
		Ok(services) => services,
		Err(unavailable) => {
			return Check::skip(
				"version_drift",
				"the workload could not be read",
				unavailable.reason(),
			);
		}
	};

	let all_facts = facts_for(ctx.runtime.as_ref(), &services).await;

	let mut running = Vec::new();
	let mut unreadable: Option<String> = None;
	for (service, facts) in services.into_iter().zip(all_facts) {
		// A service whose facts could not be read at all says nothing about the
		// version it is on, which is the same absence as facts that came back
		// naming none. Discarding the error here would let a runtime that can
		// list its workload but answer for none of it fall through to "nothing
		// running", which is a pass.
		let facts = match facts {
			Ok(facts) => facts,
			Err(unavailable) => {
				unreadable.get_or_insert_with(|| unavailable.reason().to_string());
				continue;
			}
		};
		match facts.version {
			Ok(Some(version)) => running.push(Running {
				duty: service.duty,
				id: service.id,
				version,
			}),
			Ok(None) => {}
			Err(ref unavailable) => {
				unreadable.get_or_insert_with(|| unavailable.reason().to_string());
			}
		}
	}

	// Not one service could name its version. We cannot judge drift, and saying
	// so is the point: a pass here would dress up a blind check as a healthy
	// system.
	if let Some(reason) = unreadable.filter(|_| running.is_empty()) {
		return unreadable_check(&reason);
	}

	let expected_versions = versions::expected_for_supervisor(supervisor, &ctx.version);

	// Only look at services in our expectations registry. Hand-started or
	// orphaned ones aren't drift; they're outside the expected set.
	let patient_portal_enabled = match ctx.db().await {
		Some(client) => bestool_tamanu::server_info::query_patient_portal_enabled(&client).await,
		None => None,
	};
	let patient_portal_instanced =
		matches!(supervisor, Supervisor::Systemd) && systemd_patient_portal_instanced().await;
	let expectations = expected(
		supervisor,
		kind,
		Some(ctx.config.as_ref()),
		patient_portal_enabled,
		patient_portal_instanced,
	);

	evaluate_drift(&running, &expected_versions, &expectations)
}

/// Broken result for when no service's version could be read. The check itself
/// couldn't run, so it says nothing about the system — that's broken, not a
/// warning (which would imply a degraded system) and not a pass (which would
/// dress up a blind check as a healthy one). Most often this is alertd lacking
/// access to the root-owned containers (see the podman-socket / privilege notes).
fn unreadable_check(reason: &str) -> Check {
	Check::broken(
		"version_drift",
		"could not read running service versions",
		format!("version drift can't be checked: {reason}"),
	)
}

/// Compare each running service's version against the one the deployment is
/// configured for. An empty `running` is a genuine "nothing running" pass —
/// distinct from the unreadable case handled by [`unreadable_check`].
fn evaluate_drift(
	running: &[Running],
	expected_versions: &ExpectedVersions,
	expectations: &[Expectation],
) -> Check {
	let mut rows: Vec<Value> = Vec::new();
	let mut drifted: Vec<String> = Vec::new();
	let mut total_running = 0usize;

	for service in running {
		let Some(exp) = expectations
			.iter()
			.find(|e| Duty::from_tamanu_service_name(e.name) == service.duty)
		else {
			// A service running a duty we don't expect (legacy, or hand-managed).
			// Not our concern.
			continue;
		};
		total_running += 1;
		let exp_v = expected_versions.for_service(exp.name);
		let actual = service.version.as_str();
		let status = versions::classify(Some(actual), exp_v);
		rows.push(json!({
			"duty": service.duty.to_string(),
			"service": service.id.to_string(),
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
				"{}: expected {} but running {actual}",
				service.id,
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
			.with_stat(
				Stat::gauge("running", total_running as f64).help("Expected containers running"),
			)
			.with_stat(Stat::gauge("drifted", 0.0).help("Containers on a stale version"))
	} else {
		let drifted_n = drifted.len();
		let summary = format!("{} container(s) on a stale version", drifted.len());
		Check::fail("version_drift", summary, drifted.join("; "))
			.with_detail("expected", expected_summary)
			.with_detail("instances", Value::Array(rows))
			.with_stat(
				Stat::gauge("running", total_running as f64).help("Expected containers running"),
			)
			.with_stat(
				Stat::gauge("drifted", drifted_n as f64).help("Containers on a stale version"),
			)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::check::CheckStatus;
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

	/// One running service, named the way its supervisor names it so the tests
	/// read as the deployments do.
	fn r(id: &str, version: &str) -> Running {
		// `tamanu-central-api@1.service` under systemd, `tamanu-api#0` under pm2.
		let base = id
			.split(['@', '#'])
			.next()
			.unwrap_or(id)
			.trim_end_matches(".service");
		Running {
			duty: Duty::from_tamanu_service_name(base),
			id: ServiceId::new(id),
			version: version.into(),
		}
	}

	#[test]
	fn unreadable_is_broken_not_pass() {
		// The check couldn't run at all, so it says nothing about the system:
		// broken, not a pass that would dress up a blind check as healthy.
		let check = unreadable_check("podman not found on PATH");
		assert!(matches!(check.status, CheckStatus::Broken(_)), "{check:?}");
	}

	#[test]
	fn empty_running_is_pass() {
		// The runtime answered with nothing running — genuinely fine, distinct
		// from blind.
		let exps = [exp("tamanu-central-api", Instances::NumericAtLeast(2))];
		let check = evaluate_drift(&[], &ev("v2.54.7", None), &exps);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn matching_versions_pass() {
		let exps = [exp("tamanu-central-api", Instances::NumericAtLeast(2))];
		let running = [
			r("tamanu-central-api@1.service", "v2.54.7"),
			r("tamanu-central-api@2.service", "v2.54.7"),
		];
		let check = evaluate_drift(&running, &ev("v2.54.7", None), &exps);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	/// The same deployment under either supervisor grades to the same outcome:
	/// the duty is what an expectation is matched on, so the role systemd
	/// interposes in a unit name and pm2 leaves out makes no difference.
	///
	/// spec: SUB
	#[test]
	fn drift_grades_the_same_whichever_runtime_found_the_service() {
		let systemd = [exp("tamanu-central-api", Instances::NumericAtLeast(2))];
		let pm2 = [exp("tamanu-api", Instances::NumericAtLeast(2))];

		let under_systemd = evaluate_drift(
			&[r("tamanu-central-api@1.service", "v2.54.1")],
			&ev("v2.54.7", None),
			&systemd,
		);
		let under_pm2 = evaluate_drift(&[r("tamanu-api#0", "v2.54.1")], &ev("v2.54.7", None), &pm2);

		assert!(matches!(under_systemd.status, CheckStatus::Fail(_)));
		assert!(matches!(under_pm2.status, CheckStatus::Fail(_)));
		assert_eq!(under_systemd.summary, under_pm2.summary);
	}

	#[test]
	fn drifted_frontend_fails_naming_the_service() {
		// env wants frontend v2.54.12 but the container is still on v2.54.7 —
		// exactly the case `tamanu status` couldn't see when run unprivileged.
		let exps = [
			exp("tamanu-frontend", Instances::Named(&["a", "b"])),
			exp("tamanu-central-api", Instances::NumericAtLeast(2)),
		];
		let running = [r("tamanu-frontend@a.service", "v2.54.7")];
		let check = evaluate_drift(&running, &ev("v2.54.7", Some("v2.54.12")), &exps);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains("tamanu-frontend@a"), "{reason}")
			}
			other => panic!("expected fail, got {other:?}"),
		}
	}

	#[test]
	fn bare_image_tags_are_not_drift() {
		// `/etc/tamanu/env` spells the version `v2.54.7` while the containers
		// are tagged `2.54.7`; every service is on the right version.
		let exps = [
			exp("tamanu-facility-api", Instances::NumericAtLeast(2)),
			exp("tamanu-frontend", Instances::Named(&["a", "b"])),
		];
		let running = [
			r("tamanu-facility-api@1.service", "2.54.7"),
			r("tamanu-frontend@a.service", "2.54.12"),
		];
		let check = evaluate_drift(&running, &ev("v2.54.7", Some("v2.54.12")), &exps);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn a_service_outside_the_expected_set_is_not_drift() {
		let exps = [exp("tamanu-central-api", Instances::NumericAtLeast(2))];
		let running = [r("tamanu-orphan@1.service", "v1.0.0")];
		let check = evaluate_drift(&running, &ev("v2.54.7", None), &exps);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	/// A runtime that lists its workload but cannot answer for any of it has
	/// told us nothing, and a pass would dress that up as a healthy system. The
	/// reachable case is pm2's dump fallback on Windows: the listing comes from
	/// `dump.pm2` while every facts call is refused for want of permission.
	///
	/// spec: SUB
	#[tokio::test]
	async fn a_workload_whose_facts_cannot_be_read_is_broken_not_passing() {
		use std::sync::Arc;

		use crate::runtime::{Service, ServiceId, TamanuDuty, Unavailable, fake::FakeRuntime};

		struct Unreadable(Vec<Service>);

		#[async_trait::async_trait]
		impl crate::runtime::ServiceRuntime for Unreadable {
			async fn compute(&self) -> crate::runtime::Compute {
				crate::runtime::Compute::Running
			}
			async fn services(&self) -> Result<Vec<Service>, Unavailable> {
				Ok(self.0.clone())
			}
			async fn service_facts(
				&self,
				_id: &ServiceId,
			) -> Result<crate::runtime::ServiceFacts, Unavailable> {
				Err(Unavailable::new("couldn't verify any process is alive"))
			}
		}

		let ctx = TamanuCx {
			version: "2.54.7".parse().unwrap(),
			runtime: Arc::new(Unreadable(vec![Service {
				id: ServiceId::new("tamanu-api#0"),
				duty: Duty::Tamanu(TamanuDuty::Api),
				slot: None,
				scheduled: true,
			}])),
			..crate::checks::test_support::facility_ctx()
		};

		let check = run(ctx).await;
		match &check.status {
			CheckStatus::Broken(reason) => assert!(
				reason.contains("couldn't verify any process is alive"),
				"the runtime's own reason should carry through: {reason}"
			),
			other => panic!("expected broken, got {other:?}"),
		}

		// The positive control: a runtime that lists nothing really has nothing
		// running, and that is a pass.
		let quiet = TamanuCx {
			version: "2.54.7".parse().unwrap(),
			runtime: Arc::new(FakeRuntime::empty()),
			..crate::checks::test_support::facility_ctx()
		};
		assert!(matches!(run(quiet).await.status, CheckStatus::Pass));
	}
}
