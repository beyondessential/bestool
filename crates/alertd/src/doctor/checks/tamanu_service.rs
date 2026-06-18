use serde_json::{Value, json};

use bestool_tamanu::{
	pm2,
	services::{
		Expectation, ExpectedState, Instances, Supervisor, expected, parse_systemd_unit,
		systemd_patient_portal_instanced,
	},
	systemd,
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
			"tamanu_service",
			"service check skipped on this platform",
			"no supervisor support on this platform",
		);
	};

	// Patient-portal expectation is gated on Tamanu's own `features.patientPortal`
	// DB setting. Without a DB client (e.g. unreachable), pass `None` so the
	// expectation surfaces as Unknown rather than a false-negative Down.
	let patient_portal_enabled = match ctx.db.as_deref() {
		Some(client) => bestool_tamanu::server_info::query_patient_portal_enabled(client).await,
		None => None,
	};
	let patient_portal_instanced =
		matches!(supervisor, Supervisor::Systemd) && systemd_patient_portal_instanced().await;

	// With only a database URL and no install, the config-derived expectation
	// (the FHIR worker) can't be known, so pass `None` and let it surface as
	// Unknown. Everything else comes from the supervisor, the kind (DB-derived),
	// and the patient-portal DB setting, so it runs fine.
	let config = ctx.has_install.then(|| ctx.config.as_ref());
	let expectations = expected(
		supervisor,
		ctx.kind,
		config,
		patient_portal_enabled,
		patient_portal_instanced,
	);

	let mut pm2_source: Option<pm2::Source> = None;
	let mut discovered = match supervisor {
		Supervisor::Systemd => match discover_systemd().await {
			Ok(d) => d,
			Err(err) => {
				return Check::skip("tamanu_service", "systemd unavailable", err)
					.with_detail("supervisor", "systemd");
			}
		},
		Supervisor::Pm2 => match discover_pm2() {
			Ok((d, source)) => {
				pm2_source = Some(source);
				d
			}
			Err(err) => {
				return Check::warning(
					"tamanu_service",
					"pm2 status could not be queried",
					format!(
						"pm2 unavailable ({err}); services may be running but we can't tell from this user. Run elevated to confirm."
					),
				)
				.with_detail("supervisor", "pm2");
			}
		},
	};

	if let Some(check) = pm2_dump_fallback_indeterminate(pm2_source, &discovered) {
		return check;
	}

	if matches!(supervisor, Supervisor::Systemd) {
		let candidates: Vec<String> = expectations
			.iter()
			.filter(|e| matches!(e.state, ExpectedState::Down))
			.map(|e| format!("{}.service", e.name))
			.collect();
		let enabled = systemd::collect_enabled(candidates).await;
		reconcile_down_with_enabled(&expectations, &mut discovered, |unit| {
			enabled.contains(unit)
		});
	}

	evaluate_with_source(supervisor, &expectations, &discovered, pm2_source)
}

/// Decide whether we should bail out as "indeterminate" before evaluating
/// expectations.
///
/// When the pm2 CLI is unreachable and we fall back to reading `dump.pm2`,
/// we lose the only source of truth for *which* processes are actually
/// running — running=false then just means "we couldn't read that pid file"
/// or "we couldn't see those processes in the OS table", both of which are
/// classic permission symptoms on Windows. Reporting FAIL here would lie:
/// the services are probably fine, we just can't tell. Warn instead so the
/// operator knows to re-run elevated.
fn pm2_dump_fallback_indeterminate(
	pm2_source: Option<pm2::Source>,
	discovered: &[Discovered],
) -> Option<Check> {
	if matches!(pm2_source, Some(pm2::Source::Dump))
		&& !discovered.is_empty()
		&& discovered.iter().all(|d| !d.running)
	{
		Some(
			Check::warning(
				"tamanu_service",
				"pm2 process state indeterminate",
				"read pm2's dump file but couldn't verify any process is alive — likely a permissions issue (try running elevated)",
			)
			.with_detail("supervisor", "pm2")
			.with_detail("pm2_source", pm2::Source::Dump.as_str()),
		)
	} else {
		None
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Discovered {
	/// Base name without `@instance` or `.service`.
	name: String,
	/// Whatever follows `@`, if anything.
	instance: Option<String>,
	/// Is the unit/process currently up?
	running: bool,
	/// Is the unit "present" beyond just running? For systemd this means
	/// loaded (which includes inactive-but-loaded — typically enabled). For
	/// pm2 we equate it with "is in the jlist".
	present: bool,
	/// Identifier to show in diagnostics (e.g. `tamanu-foo@1.service`).
	raw: String,
}

async fn discover_systemd() -> Result<Vec<Discovered>, String> {
	let units = systemd::list_units(&["tamanu-*.service"])
		.await
		.map_err(|e| e.to_string())?;
	let mut out = Vec::new();
	for u in units {
		let Some((base, instance)) = parse_systemd_unit(&u.name) else {
			continue;
		};
		out.push(Discovered {
			name: base.to_string(),
			instance: instance.map(str::to_string),
			running: u.running(),
			present: true,
			raw: u.name,
		});
	}
	Ok(out)
}

fn discover_pm2() -> Result<(Vec<Discovered>, pm2::Source), String> {
	let (procs, source) = pm2::list()?;
	let mut out = Vec::new();
	for p in procs {
		if !p.name.starts_with("tamanu-") {
			continue;
		}
		let raw = match p.pm_id {
			Some(id) => format!("{}#{id}", p.name),
			None => p.name.clone(),
		};
		out.push(Discovered {
			name: p.name,
			instance: None,
			running: p.running,
			present: true,
			raw,
		});
	}
	Ok((out, source))
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

/// Cross-reference Down expectations against `is-enabled` to handle the two
/// cases `list-units` alone can't disambiguate:
///
/// - Unit not in `list-units` but *is* enabled → add it as a stopped+enabled
///   `Discovered` so it gets flagged FORBIDDEN. Catches the rare
///   `enabled-but-not-loaded` state (operator enabled the unit but hasn't
///   started it or rebooted yet).
/// - Unit in `list-units` (loaded) but stopped *and* disabled → drop it.
///   Loaded-but-stopped is just systemd memory: after `systemctl stop` the
///   unit can stay loaded until the next `daemon-reload`. Combined with
///   `disabled`, it's effectively absent — it won't auto-start, has no
///   running process, and the operator has clearly indicated they don't want
///   it. Reporting FORBIDDEN here would be a false positive.
fn reconcile_down_with_enabled(
	expectations: &[Expectation],
	discovered: &mut Vec<Discovered>,
	is_enabled: impl Fn(&str) -> bool,
) {
	for exp in expectations {
		if !matches!(exp.state, ExpectedState::Down) {
			continue;
		}
		let unit = format!("{}.service", exp.name);
		let pos = discovered.iter().position(|d| d.name == exp.name);
		match pos {
			None => {
				if is_enabled(&unit) {
					discovered.push(Discovered {
						name: exp.name.to_string(),
						instance: None,
						running: false,
						present: true,
						raw: format!("{}.service (enabled)", exp.name),
					});
				}
			}
			Some(idx) if !discovered[idx].running => {
				if !is_enabled(&unit) {
					discovered.remove(idx);
				}
			}
			Some(_) => {}
		}
	}
}

fn match_expectation(
	supervisor: Supervisor,
	exp: &Expectation,
	discovered: &[Discovered],
) -> (Outcome, Vec<usize>) {
	let matched_idx: Vec<usize> = discovered
		.iter()
		.enumerate()
		.filter(|(_, d)| {
			d.name == exp.name
				&& exp
					.instances
					.admits_instance(supervisor, d.instance.as_deref())
		})
		.map(|(i, _)| i)
		.collect();

	match exp.state {
		ExpectedState::Unknown => {
			let units: Vec<String> = matched_idx
				.iter()
				.map(|i| discovered[*i].raw.clone())
				.collect();
			(Outcome::Indeterminate { discovered: units }, matched_idx)
		}
		ExpectedState::Down => {
			if matched_idx.is_empty() {
				(Outcome::Ok, matched_idx)
			} else {
				let units: Vec<String> = matched_idx
					.iter()
					.map(|i| discovered[*i].raw.clone())
					.collect();
				(Outcome::Forbidden { units }, matched_idx)
			}
		}
		ExpectedState::Up => {
			if matched_idx.is_empty() {
				return (Outcome::Missing, matched_idx);
			}
			let running: Vec<&Discovered> = matched_idx
				.iter()
				.map(|i| &discovered[*i])
				.filter(|d| d.running)
				.collect();
			let not_running: Vec<String> = matched_idx
				.iter()
				.map(|i| &discovered[*i])
				.filter(|d| !d.running)
				.map(|d| d.raw.clone())
				.collect();

			let needed = exp.instances.min_count();
			let missing_named = match &exp.instances {
				Instances::Named(names) => names
					.iter()
					.filter(|n| {
						!matched_idx.iter().any(|i| {
							discovered[*i].running
								&& discovered[*i].instance.as_deref() == Some(**n)
						})
					})
					.map(|n| format!("{}@{}", exp.name, n))
					.collect(),
				_ => Vec::new(),
			};

			if running.len() >= needed && missing_named.is_empty() {
				(Outcome::Ok, matched_idx)
			} else {
				(
					Outcome::Shortfall {
						running: running.len(),
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
		.map(|(d, _)| d.raw.clone())
		.collect();

	let supervisor_label = match supervisor {
		Supervisor::Systemd => "systemd",
		Supervisor::Pm2 => "pm2",
	};

	let services_json: Value = Value::Array(
		discovered
			.iter()
			.map(|d| {
				json!({
					"name": d.name,
					"instance": d.instance,
					"running": d.running,
					"present": d.present,
					"raw": d.raw,
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

fn evaluate_with_source(
	supervisor: Supervisor,
	expectations: &[Expectation],
	discovered: &[Discovered],
	pm2_source: Option<pm2::Source>,
) -> Check {
	let check = evaluate(supervisor, expectations, discovered);
	match pm2_source {
		Some(s) => check.with_detail("pm2_source", s.as_str()),
		None => check,
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
	use crate::doctor::check::CheckStatus;

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

	fn d(name: &str, instance: Option<&str>, running: bool) -> Discovered {
		let raw = match instance {
			Some(i) => format!("{name}@{i}.service"),
			None => format!("{name}.service"),
		};
		Discovered {
			name: name.to_string(),
			instance: instance.map(str::to_string),
			running,
			present: true,
			raw,
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

	#[test]
	fn reconcile_drops_stopped_and_disabled_down_unit() {
		// `list-units --all` reported a stopped tamanu-patientportal.service
		// (loaded but inactive), and the unit is also disabled. That's the
		// "operator stopped and disabled a service we no longer expect" case
		// — should be dropped before evaluation so it doesn't trigger
		// FORBIDDEN.
		let exps = vec![portal_down_exp()];
		let mut discovered = vec![d("tamanu-patientportal", None, false)];
		reconcile_down_with_enabled(&exps, &mut discovered, |_unit| false);
		assert!(
			discovered.is_empty(),
			"stopped+disabled unit should be dropped: {discovered:?}",
		);
	}

	#[test]
	fn reconcile_keeps_stopped_but_enabled_down_unit() {
		// Stopped but still enabled = "will auto-start at next boot". That's
		// the case the check exists to catch — keep it as discovered so the
		// evaluator marks it FORBIDDEN.
		let exps = vec![portal_down_exp()];
		let mut discovered = vec![d("tamanu-patientportal", None, false)];
		reconcile_down_with_enabled(&exps, &mut discovered, |_unit| true);
		assert_eq!(discovered.len(), 1);
	}

	#[test]
	fn reconcile_keeps_running_down_unit_regardless_of_is_enabled() {
		// Running services are unambiguously present; the is-enabled probe
		// shouldn't even fire for them.
		let exps = vec![portal_down_exp()];
		let mut discovered = vec![d("tamanu-patientportal", None, true)];
		reconcile_down_with_enabled(&exps, &mut discovered, |unit| {
			panic!("is_enabled should not be called for running unit, got {unit}");
		});
		assert_eq!(discovered.len(), 1);
	}

	#[test]
	fn reconcile_adds_enabled_but_not_loaded_down_unit() {
		// Unit isn't in `list-units` output at all, but is-enabled returns
		// true — synthesise a stopped+enabled Discovered so evaluation flags
		// FORBIDDEN.
		let exps = vec![portal_down_exp()];
		let mut discovered: Vec<Discovered> = Vec::new();
		reconcile_down_with_enabled(&exps, &mut discovered, |unit| {
			unit == "tamanu-patientportal.service"
		});
		let portal = discovered
			.iter()
			.find(|d| d.name == "tamanu-patientportal")
			.expect("portal should be synthesised");
		assert!(!portal.running);
		assert!(portal.raw.contains("enabled"));
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
			Discovered {
				name: "tamanu-tasks".into(),
				instance: None,
				running: true,
				present: true,
				raw: "tamanu-tasks#0".into(),
			},
			Discovered {
				name: "tamanu-api".into(),
				instance: None,
				running: true,
				present: true,
				raw: "tamanu-api#1".into(),
			},
			Discovered {
				name: "tamanu-api".into(),
				instance: None,
				running: true,
				present: true,
				raw: "tamanu-api#2".into(),
			},
			Discovered {
				name: "tamanu-sync".into(),
				instance: None,
				running: true,
				present: true,
				raw: "tamanu-sync#3".into(),
			},
		];
		let check = evaluate(Supervisor::Pm2, &exps, &discovered);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn pm2_dump_fallback_with_all_not_running_yields_warning() {
		let discovered = vec![
			Discovered {
				name: "tamanu-api".into(),
				instance: None,
				running: false,
				present: true,
				raw: "tamanu-api".into(),
			},
			Discovered {
				name: "tamanu-tasks".into(),
				instance: None,
				running: false,
				present: true,
				raw: "tamanu-tasks".into(),
			},
		];
		let check =
			pm2_dump_fallback_indeterminate(Some(pm2::Source::Dump), &discovered).expect("warn");
		assert!(matches!(check.status, CheckStatus::Warning(_)));
	}

	#[test]
	fn pm2_dump_fallback_with_any_running_does_not_skip() {
		let discovered = vec![
			Discovered {
				name: "tamanu-api".into(),
				instance: None,
				running: true,
				present: true,
				raw: "tamanu-api".into(),
			},
			Discovered {
				name: "tamanu-tasks".into(),
				instance: None,
				running: false,
				present: true,
				raw: "tamanu-tasks".into(),
			},
		];
		assert!(pm2_dump_fallback_indeterminate(Some(pm2::Source::Dump), &discovered).is_none());
	}

	#[test]
	fn pm2_cli_source_skips_dump_fallback_heuristic() {
		// CLI is authoritative — even if everything shows down, that's the truth.
		let discovered = vec![Discovered {
			name: "tamanu-api".into(),
			instance: None,
			running: false,
			present: true,
			raw: "tamanu-api".into(),
		}];
		assert!(pm2_dump_fallback_indeterminate(Some(pm2::Source::Cli), &discovered).is_none());
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
