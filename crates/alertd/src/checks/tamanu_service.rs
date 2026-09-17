use serde_json::{Value, json};

use bestool_tamanu::services::{
	Expectation, ExpectedState, Instances, Supervisor, expected, systemd_patient_portal_instanced,
};

use super::TamanuCx;
use crate::check::Check;
use crate::runtime::{Duty, ServiceId, facts_for};

pub async fn run(ctx: TamanuCx) -> Check {
	// Which services should be up follows from the role the deployment plays.
	// A Tamanu context is only built for a Tamanu subject, so there is always
	// one to grade against.
	let kind = ctx.server_kind();

	// The expectation set still depends on the deployment's shape — a pm2
	// deployment has no frontend and no patient portal — and on a machine that
	// is the shape of its supervisor. What the supervisor no longer decides is
	// how a discovered service is matched to an expectation: that goes by duty.
	let Some(supervisor) = Supervisor::current() else {
		return Check::skip(
			"tamanu_service",
			"service check skipped on this platform",
			"no supervisor support on this platform",
		);
	};

	// Patient-portal expectation is gated on Tamanu's own `features.patientPortal`
	// DB setting. Without a DB client (e.g. unreachable), pass `None` so the
	// expectation surfaces as Unknown rather than a false-negative Down.
	let patient_portal_enabled = match ctx.db().await {
		Some(client) => bestool_tamanu::server_info::query_patient_portal_enabled(&client).await,
		None => None,
	};
	let patient_portal_instanced =
		matches!(supervisor, Supervisor::Systemd) && systemd_patient_portal_instanced().await;

	// With only a database URL and no install, the config-derived expectation
	// (the FHIR worker) can't be known, so pass `None` and let it surface as
	// Unknown. Everything else comes from the supervisor, the kind (DB-derived),
	// and the patient-portal DB setting, so it runs fine.
	let config = ctx.installed_config();
	let expectations = expected(
		supervisor,
		kind,
		config,
		patient_portal_enabled,
		patient_portal_instanced,
	);

	let services = match ctx.runtime.services().await {
		Ok(services) => services,
		Err(unavailable) => {
			return Check::skip(
				"tamanu_service",
				"the workload could not be read",
				unavailable.reason(),
			)
			.with_detail("supervisor", supervisor_label(supervisor));
		}
	};

	let all_facts = facts_for(ctx.runtime.as_ref(), &services).await;

	let mut discovered = Vec::with_capacity(services.len());
	let mut unreadable: Option<String> = None;
	for (service, facts) in services.into_iter().zip(all_facts) {
		if let Err(ref unavailable) = facts
			&& unreadable.is_none()
		{
			unreadable = Some(unavailable.reason().to_string());
		}
		discovered.push(Discovered {
			duty: service.duty,
			slot: service.slot,
			scheduled: service.scheduled,
			running: facts.as_ref().map(|f| f.up).unwrap_or(false),
			readable: facts.is_ok(),
			id: service.id,
		});
	}

	// Every service listed and not one of them readable is the substrate
	// telling us it cannot see states, not an application that is down.
	// Reporting a shortfall would be a reading we never took.
	if let Some(reason) = unreadable
		.filter(|_| !discovered.is_empty() && discovered.iter().all(|service| !service.readable))
	{
		return Check::warning("tamanu_service", "service state indeterminate", reason)
			.with_detail("supervisor", supervisor_label(supervisor));
	}

	evaluate(supervisor, &expectations, &discovered)
}

fn supervisor_label(supervisor: Supervisor) -> &'static str {
	match supervisor {
		Supervisor::Systemd => "systemd",
		Supervisor::Pm2 => "pm2",
	}
}

/// One service the substrate reported, as the grading needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Discovered {
	/// What job it does, which is what an expectation is matched on. Never the
	/// unit, process or pod name it was found by.
	duty: Duty,
	/// Which slot it occupies within its duty, where the runtime names them.
	slot: Option<String>,
	/// Whether the runtime intends to run it, whether or not it is up.
	scheduled: bool,
	running: bool,
	/// Whether the runtime could answer for it at all. A service it could not
	/// is recorded as not running so the count is not inflated, and the
	/// distinction is what keeps "we could not tell" from reading as "down".
	readable: bool,
	/// Identifier to show in diagnostics (e.g. `tamanu-foo@1.service`).
	id: ServiceId,
}

/// The duty an expectation is about.
///
/// Expectations are still written in the supervisor's names, because the
/// lifecycle commands that build `systemctl` invocations from them need those.
/// The grading reads through to the duty, so the same expectation matches
/// whichever runtime found the service.
fn duty_of(exp: &Expectation) -> Duty {
	Duty::from_tamanu_service_name(exp.name)
}

/// Per-expectation outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
	Ok,
	/// Required but no matching unit/process at all.
	Missing,
	/// Found but with fewer running instances than required.
	Shortfall {
		running: usize,
		needed: usize,
		not_running: Vec<String>,
		missing_named: Vec<String>,
	},
	/// `Down` expectation but something is present (active or loaded).
	Forbidden {
		units: Vec<String>,
	},
	/// Expectation is `Unknown` (the driving signal was unreachable). We
	/// record what's there but neither pass nor fail; the row exists so
	/// operators see that we couldn't decide for this service.
	Indeterminate {
		discovered: Vec<String>,
	},
}

fn match_expectation(
	supervisor: Supervisor,
	exp: &Expectation,
	discovered: &[Discovered],
) -> (Outcome, Vec<usize>) {
	let duty = duty_of(exp);
	let matched_idx: Vec<usize> = discovered
		.iter()
		.enumerate()
		.filter(|(_, d)| {
			d.duty == duty && exp.instances.admits_instance(supervisor, d.slot.as_deref())
		})
		.map(|(i, _)| i)
		.collect();

	match exp.state {
		ExpectedState::Unknown => {
			let units: Vec<String> = matched_idx
				.iter()
				.map(|i| discovered[*i].id.to_string())
				.collect();
			(Outcome::Indeterminate { discovered: units }, matched_idx)
		}
		ExpectedState::Down => {
			// A service neither up nor intended to run is effectively absent:
			// it will not start on its own, nothing is serving from it, and
			// whoever stopped it has said they do not want it. On systemd that
			// is a unit left loaded after a stop, which systemd forgets at the
			// next daemon-reload; flagging it would be a false positive. What
			// must be flagged is the reverse — nothing running but the runtime
			// still intending to bring it up — and that is caught here because
			// the runtime lists such a service in the first place.
			let present: Vec<usize> = matched_idx
				.iter()
				.copied()
				.filter(|i| discovered[*i].running || discovered[*i].scheduled)
				.collect();
			if present.is_empty() {
				(Outcome::Ok, matched_idx)
			} else {
				let units: Vec<String> = present
					.iter()
					.map(|i| discovered[*i].id.to_string())
					.collect();
				(Outcome::Forbidden { units }, matched_idx)
			}
		}
		ExpectedState::Up => {
			if matched_idx.is_empty() {
				return (Outcome::Missing, matched_idx);
			}
			let running = matched_idx
				.iter()
				.filter(|i| discovered[**i].running)
				.count();
			let not_running: Vec<String> = matched_idx
				.iter()
				.map(|i| &discovered[*i])
				.filter(|d| !d.running)
				.map(|d| d.id.to_string())
				.collect();

			let needed = exp.instances.min_count();
			let missing_named = match &exp.instances {
				Instances::Named(names) => names
					.iter()
					.filter(|n| {
						!matched_idx.iter().any(|i| {
							discovered[*i].running && discovered[*i].slot.as_deref() == Some(**n)
						})
					})
					.map(|n| format!("{}@{}", exp.name, n))
					.collect(),
				_ => Vec::new(),
			};

			if running >= needed && missing_named.is_empty() {
				(Outcome::Ok, matched_idx)
			} else {
				(
					Outcome::Shortfall {
						running,
						needed,
						not_running,
						missing_named,
					},
					matched_idx,
				)
			}
		}
	}
}

fn evaluate(
	supervisor: Supervisor,
	expectations: &[Expectation],
	discovered: &[Discovered],
) -> Check {
	let mut matched_any: Vec<bool> = vec![false; discovered.len()];
	let mut per_expectation: Vec<Value> = Vec::new();
	let mut diagnostics: Vec<Value> = Vec::new();
	let mut failures: Vec<String> = Vec::new();

	for exp in expectations {
		let (outcome, idxs) = match_expectation(supervisor, exp, discovered);
		for i in idxs {
			matched_any[i] = true;
		}
		per_expectation.push(json!({
			"name": exp.name,
			"state": expected_state_label(exp.state),
			"instances": instances_to_json(&exp.instances),
			"outcome": outcome_to_json(&outcome),
			"reason": exp.reason,
			"legacy": exp.legacy,
			"behind_caddy": exp.behind_caddy,
		}));

		// `Indeterminate` is the Unknown-expectation outcome: we couldn't
		// decide what should be running. That's not a failure (we never
		// claimed the actual state is wrong), so it doesn't go in the
		// failures list — but it does land in `diagnostics` so operators
		// see the row was deliberately not evaluated.
		if matches!(outcome, Outcome::Indeterminate { .. }) {
			let (actual, detail) = actual_for_outcome(exp, &outcome);
			let mut diag = json!({
				"name": exp.name,
				"expected": expected_state_label(exp.state),
				"reason": exp.reason,
				"actual": actual,
			});
			if let Some(d) = detail {
				diag["detail"] = Value::String(d);
			}
			diagnostics.push(diag);
		} else if !matches!(outcome, Outcome::Ok) {
			let (actual, detail) = actual_for_outcome(exp, &outcome);
			let expected_label = expected_state_label(exp.state);
			let mut diag = json!({
				"name": exp.name,
				"expected": expected_label,
				"reason": exp.reason,
				"actual": actual,
			});
			if let Some(ref d) = detail {
				diag["detail"] = Value::String(d.clone());
			}
			diagnostics.push(diag);

			let mut line = format!(
				"{}: expected {expected_label} ({reason}), got {actual}",
				exp.name,
				reason = exp.reason,
			);
			if let Some(d) = detail {
				line.push_str(&format!(" ({d})"));
			}
			failures.push(line);
		}
	}

	let extras: Vec<String> = discovered
		.iter()
		.zip(matched_any.iter())
		.filter(|(_, m)| !**m)
		.map(|(d, _)| d.id.to_string())
		.collect();

	let supervisor_label = supervisor_label(supervisor);

	let services_json: Value = Value::Array(
		discovered
			.iter()
			.map(|d| {
				json!({
					"duty": d.duty.to_string(),
					"slot": d.slot,
					"running": d.running,
					"scheduled": d.scheduled,
					"readable": d.readable,
					"id": d.id.to_string(),
				})
			})
			.collect(),
	);

	let summary = if failures.is_empty() {
		format!("{} expectation(s) met", expectations.len())
	} else {
		format!("{} expectation(s) unmet", failures.len())
	};

	let check = if failures.is_empty() {
		Check::pass("tamanu_service", summary)
	} else {
		Check::fail("tamanu_service", summary, failures.join("; "))
	};

	// Per-check (`health[]`) details are kept lean: a per-service diagnostic
	// list aimed at humans, plus the supervisor label. The bulky raw data
	// (full expectations, discovered units, extras, supervisor) goes into the
	// top-level status payload via `payload_extras` under `services`, so each
	// piece lives in its natural home.
	let payload_services = json!({
		"supervisor": supervisor_label,
		"expectations": Value::Array(per_expectation),
		"discovered": services_json,
		"extras": Value::Array(extras.into_iter().map(Value::String).collect()),
	});

	check
		.with_detail("supervisor", supervisor_label)
		.with_detail("diagnostics", Value::Array(diagnostics))
		.with_payload_extra("services", payload_services)
}

fn expected_state_label(s: ExpectedState) -> &'static str {
	match s {
		ExpectedState::Up => "up",
		ExpectedState::Down => "down",
		ExpectedState::Unknown => "unknown",
	}
}

fn actual_for_outcome(exp: &Expectation, outcome: &Outcome) -> (&'static str, Option<String>) {
	match outcome {
		Outcome::Ok => (expected_state_label(exp.state), None),
		Outcome::Missing => ("missing", None),
		Outcome::Shortfall {
			running,
			needed,
			not_running,
			missing_named,
		} => {
			let mut parts = vec![format!("{running}/{needed} instance(s) running")];
			if !missing_named.is_empty() {
				parts.push(format!("missing {}", missing_named.join(", ")));
			}
			if !not_running.is_empty() {
				parts.push(format!("not running: {}", not_running.join(", ")));
			}
			("partial", Some(parts.join("; ")))
		}
		Outcome::Forbidden { units } => ("up", Some(units.join(", "))),
		Outcome::Indeterminate { discovered } => {
			let detail = if discovered.is_empty() {
				None
			} else {
				Some(discovered.join(", "))
			};
			("indeterminate", detail)
		}
	}
}

fn instances_to_json(i: &Instances) -> Value {
	match i {
		Instances::Single => json!({"kind": "single"}),
		Instances::NumericAtLeast(n) => json!({"kind": "numeric_at_least", "min": n}),
		Instances::Named(xs) => json!({"kind": "named", "names": xs}),
	}
}

fn outcome_to_json(o: &Outcome) -> Value {
	match o {
		Outcome::Ok => json!({"kind": "ok"}),
		Outcome::Missing => json!({"kind": "missing"}),
		Outcome::Shortfall {
			running,
			needed,
			not_running,
			missing_named,
		} => json!({
			"kind": "shortfall",
			"running": running,
			"needed": needed,
			"not_running": not_running,
			"missing_named": missing_named,
		}),
		Outcome::Forbidden { units } => json!({"kind": "forbidden", "units": units}),
		Outcome::Indeterminate { discovered } => {
			json!({"kind": "indeterminate", "discovered": discovered})
		}
	}
}

#[cfg(test)]
mod tests {
	use bestool_tamanu::{ApiServerKind, config::TamanuConfig};

	use super::*;
	use crate::check::CheckStatus;

	fn cfg(fhir_worker: bool) -> TamanuConfig {
		let json = serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"serverFacilityIds": ["facility-x"],
			"integrations": { "fhir": { "worker": { "enabled": fhir_worker } } },
		});
		serde_json::from_value(json).unwrap()
	}

	fn central_cfg(fhir_worker: bool) -> TamanuConfig {
		let json = serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"integrations": { "fhir": { "worker": { "enabled": fhir_worker } } },
		});
		serde_json::from_value(json).unwrap()
	}

	/// A service as a runtime would report it, named the way the supervisor
	/// under it names one so the tests read as the deployments do.
	fn d(name: &str, instance: Option<&str>, running: bool) -> Discovered {
		Discovered {
			duty: Duty::from_tamanu_service_name(name),
			slot: instance.map(str::to_string),
			scheduled: true,
			running,
			readable: true,
			id: ServiceId::new(match instance {
				Some(i) => format!("{name}@{i}.service"),
				None => format!("{name}.service"),
			}),
		}
	}

	/// A service the runtime lists but does not intend to run: a systemd unit
	/// left loaded after a stop, with no symlink to bring it back.
	fn stopped_and_unscheduled(name: &str) -> Discovered {
		Discovered {
			scheduled: false,
			..d(name, None, false)
		}
	}

	#[test]
	fn happy_facility_systemd() {
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-facility-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-api", Some("2"), true),
			d("tamanu-facility-sync", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn fails_when_tasks_missing() {
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-api", Some("2"), true),
			d("tamanu-facility-sync", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		match &check.status {
			CheckStatus::Fail(reason) => assert!(
				reason.contains("tamanu-facility-tasks"),
				"reason was {reason:?}"
			),
			other => panic!("expected fail, got {other:?}"),
		}
	}

	#[test]
	fn fails_on_api_shortfall() {
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-facility-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-sync", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains("1/2"), "reason was {reason:?}");
			}
			other => panic!("{other:?}"),
		}
	}

	#[test]
	fn fails_on_frontend_named_missing() {
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-facility-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			// no @b
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-api", Some("2"), true),
			d("tamanu-facility-sync", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(
					reason.contains("tamanu-frontend@b"),
					"reason was {reason:?}"
				);
			}
			other => panic!("{other:?}"),
		}
	}

	#[test]
	fn fails_when_forbidden_facility_singleton_present() {
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-facility-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-api", Some("2"), true),
			d("tamanu-facility-sync", None, true),
			// legacy singleton that must not be present:
			d("tamanu-facility", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains("expected down"), "reason was {reason:?}");
				assert!(reason.contains("tamanu-facility"), "reason was {reason:?}");
				assert!(
					reason.contains("legacy singleton unit must not be present"),
					"reason was {reason:?}"
				);
			}
			other => panic!("{other:?}"),
		}
	}

	fn portal_down_exp() -> Expectation {
		Expectation {
			name: "tamanu-patientportal",
			instances: Instances::Single,
			state: ExpectedState::Down,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
		}
	}

	/// A service neither up nor intended to run is effectively absent: on
	/// systemd that is a unit left loaded after a stop, which flagging would
	/// make a false positive.
	#[test]
	fn a_stopped_and_unscheduled_service_is_not_forbidden() {
		let exps = [portal_down_exp()];
		let discovered = vec![stopped_and_unscheduled("tamanu-patientportal")];
		let (outcome, _) = match_expectation(Supervisor::Systemd, &exps[0], &discovered);
		assert_eq!(outcome, Outcome::Ok, "{outcome:?}");
	}

	/// Stopped but still intended to run means it comes back at the next boot,
	/// which is exactly what a `Down` expectation exists to catch.
	#[test]
	fn a_stopped_but_scheduled_service_is_forbidden() {
		let exps = [portal_down_exp()];
		let discovered = vec![d("tamanu-patientportal", None, false)];
		let (outcome, _) = match_expectation(Supervisor::Systemd, &exps[0], &discovered);
		assert!(matches!(outcome, Outcome::Forbidden { .. }), "{outcome:?}");
	}

	/// A running service is unambiguously there, whatever the runtime intends.
	#[test]
	fn a_running_service_is_forbidden_even_if_unscheduled() {
		let exps = [portal_down_exp()];
		let discovered = vec![Discovered {
			scheduled: false,
			..d("tamanu-patientportal", None, true)
		}];
		let (outcome, _) = match_expectation(Supervisor::Systemd, &exps[0], &discovered);
		assert!(matches!(outcome, Outcome::Forbidden { .. }), "{outcome:?}");
	}

	#[test]
	fn extras_recorded_but_dont_fail() {
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let mut discovered = vec![
			d("tamanu-facility-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-api", Some("2"), true),
			d("tamanu-facility-sync", None, true),
		];
		discovered.push(d("tamanu-patientportal", None, true));
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
		let services = check
			.payload_extras
			.get("services")
			.expect("services payload_extra");
		let extras = services
			.get("extras")
			.and_then(Value::as_array)
			.expect("extras array");
		assert_eq!(extras.len(), 1);
		assert_eq!(extras[0].as_str().unwrap(), "tamanu-patientportal.service");
	}

	#[test]
	fn leftover_singleton_does_not_satisfy_instanced_portal() {
		// Host mid-migration: the @a/@b template is installed (so the portal
		// expectation is instanced and Up) but only the old singleton is
		// running. The singleton must not count toward the instanced
		// requirement — the check should fail for the missing @a/@b, and the
		// singleton lands in `extras`.
		let cfg = central_cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg),
			Some(true),
			true,
		);
		let discovered = vec![
			d("tamanu-central-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-central-api", Some("1"), true),
			d("tamanu-central-api", Some("2"), true),
			d("tamanu-central-fhir-resolve", None, true),
			d("tamanu-central-fhir-refresh", None, true),
			d("tamanu-patientportal", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		match &check.status {
			CheckStatus::Fail(reason) => assert!(
				reason.contains("tamanu-patientportal"),
				"reason was {reason:?}"
			),
			other => panic!("expected fail, got {other:?}"),
		}
		let extras = check
			.payload_extras
			.get("services")
			.and_then(|s| s.get("extras"))
			.and_then(Value::as_array)
			.expect("extras array");
		assert_eq!(extras.len(), 1);
		assert_eq!(extras[0].as_str().unwrap(), "tamanu-patientportal.service");
	}

	#[test]
	fn unknown_portal_expectation_does_not_fail_check() {
		// DB unreachable → portal expectation is Unknown. The doctor must
		// not flag this as a service-check failure: we don't know what the
		// portal should be doing, so any running/stopped state is fine.
		let cfg = central_cfg(true);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg),
			None,
			false,
		);
		let discovered = vec![
			d("tamanu-central-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-central-api", Some("1"), true),
			d("tamanu-central-api", Some("2"), true),
			d("tamanu-central-fhir-resolve", None, true),
			d("tamanu-central-fhir-refresh", None, true),
			// patient portal is running; with Unknown expectation, that
			// must NOT count as a failure.
			d("tamanu-patientportal", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn central_with_fhir_requires_workers() {
		let cfg = central_cfg(true);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-central-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-central-api", Some("1"), true),
			d("tamanu-central-api", Some("2"), true),
			// fhir workers missing
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(
					reason.contains("tamanu-central-fhir-resolve"),
					"reason was {reason:?}"
				);
				assert!(
					reason.contains("tamanu-central-fhir-refresh"),
					"reason was {reason:?}"
				);
			}
			other => panic!("{other:?}"),
		}
	}

	#[test]
	fn central_without_fhir_doesnt_require_workers() {
		// `central_cfg(false)` has no `patientPortal` block, so the doctor
		// expects `tamanu-patientportal` Down — i.e. absent from `discovered`
		// is the pass case.
		let cfg = central_cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-central-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-central-api", Some("1"), true),
			d("tamanu-central-api", Some("2"), true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn pm2_facility_happy() {
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Pm2,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-tasks", None, true),
			d("tamanu-api", None, true),
			d("tamanu-api", None, true),
			d("tamanu-sync", None, true),
		];
		let check = evaluate(Supervisor::Pm2, &exps, &discovered);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn not_running_listed_as_diagnosis() {
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-facility-tasks", None, false), // not running
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-api", Some("2"), true),
			d("tamanu-facility-sync", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains("not running"), "reason was {reason:?}");
				assert!(
					reason.contains("tamanu-facility-tasks"),
					"reason was {reason:?}"
				);
			}
			other => panic!("{other:?}"),
		}
	}

	#[test]
	fn diagnostics_carry_per_service_reason_and_state() {
		// Patient-portal Down with the service actually running is the case
		// that triggered this restructuring: the wire output should make it
		// trivial to read "expected down (DB setting features.patientPortal is
		// false), got up (tamanu-patientportal.service)" rather than parsing
		// expectations + services arrays.
		let cfg = central_cfg(true);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-central-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-central-api", Some("1"), true),
			d("tamanu-central-api", Some("2"), true),
			d("tamanu-central-fhir-resolve", None, true),
			d("tamanu-central-fhir-refresh", None, true),
			d("tamanu-patientportal", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		let diagnostics = check
			.details
			.get("diagnostics")
			.and_then(Value::as_array)
			.expect("diagnostics array");
		// Only the failing portal entry should appear — everything else
		// matched its expectation and lives only in the top-level raw payload.
		assert_eq!(diagnostics.len(), 1);
		let portal = &diagnostics[0];
		assert_eq!(
			portal.get("name").and_then(Value::as_str),
			Some("tamanu-patientportal")
		);
		assert_eq!(portal.get("expected").and_then(Value::as_str), Some("down"));
		assert_eq!(portal.get("actual").and_then(Value::as_str), Some("up"));
		assert_eq!(
			portal.get("reason").and_then(Value::as_str),
			Some("DB setting features.patientPortal is false")
		);
		assert_eq!(
			portal.get("detail").and_then(Value::as_str),
			Some("tamanu-patientportal.service")
		);
	}

	#[test]
	fn diagnostics_empty_when_everything_matches() {
		// Happy path: no per-service diagnostics in the health[] entry. The
		// raw inventory is still in the top-level payload under `services`
		// for anyone who wants to audit what was checked.
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-facility-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-api", Some("2"), true),
			d("tamanu-facility-sync", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		assert!(matches!(check.status, CheckStatus::Pass));
		let diagnostics = check
			.details
			.get("diagnostics")
			.and_then(Value::as_array)
			.expect("diagnostics array");
		assert!(diagnostics.is_empty(), "{diagnostics:?}");
		// Raw inventory is still available in the top-level payload.
		assert!(check.payload_extras.get("services").is_some());
	}

	#[test]
	fn raw_data_lives_in_payload_extras_not_check_details() {
		// Bulky data (raw expectations / discovered units / supervisor /
		// extras) belongs in the top-level status payload via
		// `payload_extras["services"]`, not under per-check `details`. Keeps
		// the `health[]` entry focused on human-readable diagnostics.
		let cfg = cfg(false);
		let exps = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg),
			Some(false),
			false,
		);
		let discovered = vec![
			d("tamanu-facility-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-api", Some("2"), true),
			d("tamanu-facility-sync", None, true),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);

		assert!(!check.details.contains_key("expectations"));
		assert!(!check.details.contains_key("extras"));
		// `services` in details used to be the raw discovered-units array;
		// it now lives in the top-level payload under that same key.
		assert!(!check.details.contains_key("services"));

		let services = check
			.payload_extras
			.get("services")
			.expect("services payload extra");
		assert_eq!(
			services.get("supervisor").and_then(Value::as_str),
			Some("systemd")
		);
		let raw_exps = services
			.get("expectations")
			.and_then(Value::as_array)
			.expect("raw expectations");
		assert!(!raw_exps.is_empty());
		// Each raw expectation carries its reason so the payload is
		// self-describing without the diagnostics list.
		assert!(
			raw_exps
				.iter()
				.all(|e| e.get("reason").and_then(Value::as_str).is_some())
		);
		assert!(services.get("discovered").is_some());
		assert!(services.get("extras").is_some());
	}
}
