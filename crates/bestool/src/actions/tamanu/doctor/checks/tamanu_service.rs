use std::process::Command;

use serde_json::{Value, json};

use super::CheckContext;
use crate::actions::tamanu::{
	ApiServerKind,
	doctor::check::Check,
	services::{ExpectedState, Expectation, Instances, Supervisor, expected, parse_systemd_unit},
};

pub async fn run(ctx: CheckContext) -> Check {
	let supervisor = if cfg!(target_os = "linux") {
		Supervisor::Systemd
	} else if cfg!(target_os = "windows") {
		Supervisor::Pm2
	} else {
		return Check::pass("tamanu_service", "service check skipped on this platform")
			.with_detail("skipped", true);
	};

	let kind = if ctx.config.is_facility() {
		ApiServerKind::Facility
	} else {
		ApiServerKind::Central
	};
	let expectations = expected(supervisor, kind, &ctx.config);

	let discovered = match supervisor {
		Supervisor::Systemd => match discover_systemd() {
			Ok(d) => d,
			Err(err) => {
				return Check::fail("tamanu_service", "systemctl unavailable", err)
					.with_detail("supervisor", "systemd");
			}
		},
		Supervisor::Pm2 => match discover_pm2() {
			Ok(d) => d,
			Err(err) => {
				return Check::fail("tamanu_service", "pm2 unavailable", err)
					.with_detail("supervisor", "pm2");
			}
		},
	};

	evaluate(supervisor, &expectations, &discovered)
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

fn discover_systemd() -> Result<Vec<Discovered>, String> {
	let output = Command::new("systemctl")
		.args([
			"list-units",
			"--type=service",
			"--all",
			"--no-legend",
			"--plain",
			"--no-pager",
			"tamanu-*.service",
		])
		.output()
		.map_err(|e| e.to_string())?;

	let stdout = String::from_utf8_lossy(&output.stdout);
	let mut out = Vec::new();
	for line in stdout.lines() {
		let mut parts = line.split_whitespace();
		let (Some(unit), Some(load), Some(active), Some(sub)) =
			(parts.next(), parts.next(), parts.next(), parts.next())
		else {
			continue;
		};
		if load == "not-found" {
			continue;
		}
		let Some((base, instance)) = parse_systemd_unit(unit) else {
			continue;
		};
		let running = active == "active" && (sub == "running" || sub == "exited");
		out.push(Discovered {
			name: base.to_string(),
			instance: instance.map(str::to_string),
			running,
			present: true,
			raw: unit.to_string(),
		});
	}
	Ok(out)
}

fn discover_pm2() -> Result<Vec<Discovered>, String> {
	let output = Command::new("pm2")
		.arg("jlist")
		.output()
		.map_err(|e| e.to_string())?;
	if !output.status.success() {
		return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
	}
	let parsed: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
	let mut out = Vec::new();
	if let Some(procs) = parsed.as_array() {
		for p in procs {
			let Some(name) = p["name"].as_str() else {
				continue;
			};
			if !name.starts_with("tamanu-") {
				continue;
			}
			let state = p["pm2_env"]["status"].as_str().unwrap_or("unknown");
			let pm_id = p["pm_id"].as_i64();
			let raw = match pm_id {
				Some(id) => format!("{name}#{id}"),
				None => name.to_string(),
			};
			out.push(Discovered {
				name: name.to_string(),
				instance: None,
				running: state == "online",
				present: true,
				raw,
			});
		}
	}
	Ok(out)
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
	Forbidden { units: Vec<String> },
}

fn match_expectation(exp: &Expectation, discovered: &[Discovered]) -> (Outcome, Vec<usize>) {
	let matched_idx: Vec<usize> = discovered
		.iter()
		.enumerate()
		.filter(|(_, d)| d.name == exp.name && exp.instances.admits_instance(d.instance.as_deref()))
		.map(|(i, _)| i)
		.collect();

	match exp.state {
		ExpectedState::Down => {
			if matched_idx.is_empty() {
				(Outcome::Ok, matched_idx)
			} else {
				let units: Vec<String> =
					matched_idx.iter().map(|i| discovered[*i].raw.clone()).collect();
				(Outcome::Forbidden { units }, matched_idx)
			}
		}
		ExpectedState::Up => {
			if matched_idx.is_empty() {
				return (Outcome::Missing, matched_idx);
			}
			let running: Vec<&Discovered> =
				matched_idx.iter().map(|i| &discovered[*i]).filter(|d| d.running).collect();
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
	let mut failures: Vec<String> = Vec::new();

	for exp in expectations {
		let (outcome, idxs) = match_expectation(exp, discovered);
		for i in idxs {
			matched_any[i] = true;
		}
		per_expectation.push(json!({
			"name": exp.name,
			"state": match exp.state {
				ExpectedState::Up => "up",
				ExpectedState::Down => "down",
			},
			"instances": instances_to_json(&exp.instances),
			"outcome": outcome_to_json(&outcome),
		}));
		match &outcome {
			Outcome::Ok => {}
			Outcome::Missing => failures.push(format!("missing {}", exp.name)),
			Outcome::Shortfall {
				running,
				needed,
				not_running,
				missing_named,
			} => {
				if !missing_named.is_empty() {
					failures.push(format!(
						"{}: missing instance(s) {}",
						exp.name,
						missing_named.join(", ")
					));
				} else if !not_running.is_empty() {
					failures.push(format!(
						"{}: not running ({})",
						exp.name,
						not_running.join(", ")
					));
				} else {
					failures.push(format!(
						"{}: only {running}/{needed} instance(s) running",
						exp.name
					));
				}
			}
			Outcome::Forbidden { units } => {
				failures.push(format!("forbidden present: {}", units.join(", ")));
			}
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

	check.with_detail("supervisor", supervisor_label)
		.with_detail("expectations", Value::Array(per_expectation))
		.with_detail("services", services_json)
		.with_detail(
			"extras",
			Value::Array(extras.into_iter().map(Value::String).collect()),
		)
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
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::actions::tamanu::config::TamanuConfig;
	use crate::actions::tamanu::doctor::check::CheckStatus;

	fn cfg(fhir_worker: bool) -> TamanuConfig {
		let json = serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"serverFacilityIds": ["facility-x"],
			"fhir": { "worker": { "enabled": fhir_worker } },
		});
		serde_json::from_value(json).unwrap()
	}

	fn central_cfg(fhir_worker: bool) -> TamanuConfig {
		let json = serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"fhir": { "worker": { "enabled": fhir_worker } },
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
		let exps = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg);
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
		let exps = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg);
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
		let exps = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg);
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
		let exps = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg);
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
		let exps = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg);
		let discovered = vec![
			d("tamanu-facility-tasks", None, true),
			d("tamanu-frontend", Some("a"), true),
			d("tamanu-frontend", Some("b"), true),
			d("tamanu-facility-api", Some("1"), true),
			d("tamanu-facility-api", Some("2"), true),
			d("tamanu-facility-sync", None, true),
			// legacy singleton that must not be present:
			d("tamanu-facility", None, false),
		];
		let check = evaluate(Supervisor::Systemd, &exps, &discovered);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains("forbidden"), "reason was {reason:?}");
				assert!(
					reason.contains("tamanu-facility"),
					"reason was {reason:?}"
				);
			}
			other => panic!("{other:?}"),
		}
	}

	#[test]
	fn extras_recorded_but_dont_fail() {
		let cfg = cfg(false);
		let exps = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg);
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
		let extras = check.details.get("extras").unwrap().as_array().unwrap();
		assert_eq!(extras.len(), 1);
		assert_eq!(
			extras[0].as_str().unwrap(),
			"tamanu-patientportal.service"
		);
	}

	#[test]
	fn central_with_fhir_requires_workers() {
		let cfg = central_cfg(true);
		let exps = expected(Supervisor::Systemd, ApiServerKind::Central, &cfg);
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
		let cfg = central_cfg(false);
		let exps = expected(Supervisor::Systemd, ApiServerKind::Central, &cfg);
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
		let exps = expected(Supervisor::Pm2, ApiServerKind::Facility, &cfg);
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
	fn not_running_listed_as_diagnosis() {
		let cfg = cfg(false);
		let exps = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg);
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
				assert!(
					reason.contains("not running"),
					"reason was {reason:?}"
				);
				assert!(
					reason.contains("tamanu-facility-tasks"),
					"reason was {reason:?}"
				);
			}
			other => panic!("{other:?}"),
		}
	}
}
