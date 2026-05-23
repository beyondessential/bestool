//! Declarative description of which Tamanu services should be up (or down)
//! for a given deployment.
//!
//! The shape of this module is driven by what `tamanu doctor` needs to verify
//! health, but it's intended to be reusable by other subcommands (rolling
//! restarts, find, etc.).
//!
//! ## Naming conventions
//!
//! - pm2: `tamanu-${thing}`. pm2 doesn't carry an `@instance` suffix; when a
//!   logical service runs N copies, N processes share the same `name`.
//! - systemd: `tamanu-${kind}-${thing}` for kind-scoped units, with instances
//!   exposed as template suffixes `@1`, `@2`, … Exceptions:
//!   - `frontend` lives at `tamanu-frontend` (no kind prefix) with named
//!     instances `@a` and `@b`.
//!   - `tamanu-facility` is a literal singleton unit (no kind prefix, no
//!     `${thing}` segment) that's a leftover from older deployments and must
//!     not be active or enabled on current ones.

use crate::ApiServerKind;
use crate::config::TamanuConfig;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Supervisor {
	Systemd,
	Pm2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
	/// Concrete service base name as the supervisor sees it.
	pub name: &'static str,
	pub instances: Instances,
	pub state: ExpectedState,
	/// Availability constraint when restarting. Only meaningful for
	/// `ExpectedState::Up` services.
	pub criticality: Criticality,
}

/// Whether a service must keep at least one instance up at all times.
///
/// Drives the restart strategy: `Critical` rolls one instance at a time
/// with a readiness probe between each; `Background` restarts in bulk.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Criticality {
	/// Must always have at least one instance up. API and frontend.
	Critical,
	/// No availability constraint. Tasks, sync, fhir-*.
	Background,
}

/// How the instances of a logical service are arranged.
///
/// For pm2 (which has no `@instance` notation), `NumericAtLeast(n)` means
/// "at least n processes share this name"; `Named(_)` means "this many
/// processes share this name" (the names themselves are ignored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instances {
	/// Singleton: one unit/process, no `@instance` suffix.
	Single,
	/// At least N instances, named numerically `@1`, `@2`, …
	NumericAtLeast(usize),
	/// Exactly these instance suffixes, e.g. `["a", "b"]`.
	Named(&'static [&'static str]),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExpectedState {
	Up,
	/// Must NOT be active and (on systemd) must NOT be enabled.
	Down,
}

impl Instances {
	/// Lower bound on the number of running instances required to satisfy the
	/// expectation.
	pub fn min_count(&self) -> usize {
		match self {
			Instances::Single => 1,
			Instances::NumericAtLeast(n) => *n,
			Instances::Named(xs) => xs.len(),
		}
	}

	/// The minimum set of systemd units that must be running to satisfy this
	/// expectation. `NumericAtLeast(n)` enumerates `@1..@n`; `Named(xs)`
	/// enumerates each suffix; `Single` is just the bare unit.
	///
	/// Used by `tamanu start` to compute which units to bring up. Not
	/// meaningful for pm2.
	pub fn required_systemd_units(&self, base: &str) -> Vec<String> {
		match self {
			Instances::Single => vec![format!("{base}.service")],
			Instances::NumericAtLeast(n) => (1..=*n)
				.map(|i| format!("{base}@{i}.service"))
				.collect(),
			Instances::Named(xs) => xs.iter().map(|s| format!("{base}@{s}.service")).collect(),
		}
	}

	/// Whether a discovered `instance` (the bit after `@`, or `None` if there
	/// was no `@` suffix) is one this pattern admits.
	pub fn admits_instance(&self, instance: Option<&str>) -> bool {
		match (self, instance) {
			(Instances::Single, None) => true,
			(Instances::Single, Some(_)) => false,
			(Instances::NumericAtLeast(_), None) => true, // pm2 case
			(Instances::NumericAtLeast(_), Some(s)) => s.chars().all(|c| c.is_ascii_digit()),
			(Instances::Named(_), None) => true, // pm2 case
			(Instances::Named(xs), Some(s)) => xs.contains(&s),
		}
	}
}

/// All services expected to exist (or be absent) for this deployment.
pub fn expected(
	supervisor: Supervisor,
	kind: ApiServerKind,
	config: &TamanuConfig,
) -> Vec<Expectation> {
	let mut out = Vec::new();

	let tasks_name = match supervisor {
		Supervisor::Pm2 => "tamanu-tasks",
		Supervisor::Systemd => match kind {
			ApiServerKind::Central => "tamanu-central-tasks",
			ApiServerKind::Facility => "tamanu-facility-tasks",
		},
	};
	out.push(Expectation {
		name: tasks_name,
		instances: Instances::Single,
		state: ExpectedState::Up,
		criticality: Criticality::Background,
	});

	if matches!(supervisor, Supervisor::Systemd) {
		out.push(Expectation {
			name: "tamanu-frontend",
			instances: Instances::Named(&["a", "b"]),
			state: ExpectedState::Up,
			criticality: Criticality::Critical,
		});
		out.push(Expectation {
			name: "tamanu-facility",
			instances: Instances::Single,
			state: ExpectedState::Down,
			// criticality is unused for Down; Background is the harmless default.
			criticality: Criticality::Background,
		});
	}

	let api_name = match supervisor {
		Supervisor::Pm2 => "tamanu-api",
		Supervisor::Systemd => match kind {
			ApiServerKind::Central => "tamanu-central-api",
			ApiServerKind::Facility => "tamanu-facility-api",
		},
	};
	out.push(Expectation {
		name: api_name,
		instances: Instances::NumericAtLeast(2),
		state: ExpectedState::Up,
		criticality: Criticality::Critical,
	});

	match kind {
		ApiServerKind::Central => {
			if config.fhir_worker_enabled() {
				let (resolve, refresh) = match supervisor {
					Supervisor::Pm2 => ("tamanu-fhir-resolve", "tamanu-fhir-refresh"),
					Supervisor::Systemd => {
						("tamanu-central-fhir-resolve", "tamanu-central-fhir-refresh")
					}
				};
				out.push(Expectation {
					name: resolve,
					instances: Instances::Single,
					state: ExpectedState::Up,
					criticality: Criticality::Background,
				});
				out.push(Expectation {
					name: refresh,
					instances: Instances::Single,
					state: ExpectedState::Up,
					criticality: Criticality::Background,
				});
			}
		}
		ApiServerKind::Facility => {
			let sync_name = match supervisor {
				Supervisor::Pm2 => "tamanu-sync",
				Supervisor::Systemd => "tamanu-facility-sync",
			};
			out.push(Expectation {
				name: sync_name,
				instances: Instances::Single,
				state: ExpectedState::Up,
				criticality: Criticality::Background,
			});
		}
	}

	out
}

/// Filter an expectation set by zero or more substring patterns.
///
/// - Empty `names`: returns every expectation unchanged.
/// - Otherwise: an expectation matches if **any** name in `names` is a
///   substring of the expectation's name.
///
/// Returns an error if any name in `names` matched zero expectations
/// (typo safety in multi-name invocations).
pub fn match_names<'a>(
	expectations: &'a [Expectation],
	names: &[&str],
) -> miette::Result<Vec<&'a Expectation>> {
	use miette::bail;

	if names.is_empty() {
		return Ok(expectations.iter().collect());
	}

	let unmatched: Vec<&str> = names
		.iter()
		.copied()
		.filter(|name| !expectations.iter().any(|e| e.name.contains(name)))
		.collect();
	if !unmatched.is_empty() {
		let available: Vec<&str> = expectations.iter().map(|e| e.name).collect();
		bail!(
			"no service matches: {}; available names are: {}",
			unmatched.join(", "),
			available.join(", "),
		);
	}

	let matched: Vec<&Expectation> = expectations
		.iter()
		.filter(|e| names.iter().any(|name| e.name.contains(name)))
		.collect();
	Ok(matched)
}

/// Parse a systemd unit name (`tamanu-foo@1.service`, `tamanu-foo.service`,
/// `tamanu-foo`) into its base name and optional instance.
///
/// Returns `None` if the input doesn't start with `tamanu-`.
pub fn parse_systemd_unit(unit: &str) -> Option<(&str, Option<&str>)> {
	let unit = unit.strip_suffix(".service").unwrap_or(unit);
	if !unit.starts_with("tamanu-") {
		return None;
	}
	if let Some((base, instance)) = unit.split_once('@') {
		Some((base, Some(instance)))
	} else {
		Some((unit, None))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn cfg(fhir_worker: bool) -> TamanuConfig {
		let json = serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"fhir": { "worker": { "enabled": fhir_worker } },
		});
		serde_json::from_value(json).unwrap()
	}

	fn names(es: &[Expectation]) -> Vec<&str> {
		es.iter().map(|e| e.name).collect()
	}

	#[test]
	fn facility_pm2_no_fhir() {
		let es = expected(Supervisor::Pm2, ApiServerKind::Facility, &cfg(false));
		assert_eq!(
			names(&es),
			vec!["tamanu-tasks", "tamanu-api", "tamanu-sync"]
		);
		// no Down expectations on pm2
		assert!(es.iter().all(|e| e.state == ExpectedState::Up));
	}

	#[test]
	fn central_pm2_no_fhir_skips_fhir_units() {
		let es = expected(Supervisor::Pm2, ApiServerKind::Central, &cfg(false));
		assert_eq!(names(&es), vec!["tamanu-tasks", "tamanu-api"]);
	}

	#[test]
	fn central_pm2_with_fhir_adds_resolve_and_refresh() {
		let es = expected(Supervisor::Pm2, ApiServerKind::Central, &cfg(true));
		assert_eq!(
			names(&es),
			vec![
				"tamanu-tasks",
				"tamanu-api",
				"tamanu-fhir-resolve",
				"tamanu-fhir-refresh",
			]
		);
	}

	#[test]
	fn facility_systemd_includes_frontend_and_forbids_legacy_facility() {
		let es = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg(false));
		assert_eq!(
			names(&es),
			vec![
				"tamanu-facility-tasks",
				"tamanu-frontend",
				"tamanu-facility",
				"tamanu-facility-api",
				"tamanu-facility-sync",
			]
		);
		let forbidden = es
			.iter()
			.find(|e| e.name == "tamanu-facility")
			.expect("forbidden expectation");
		assert_eq!(forbidden.state, ExpectedState::Down);
	}

	#[test]
	fn central_systemd_with_fhir_uses_central_prefixed_units() {
		let es = expected(Supervisor::Systemd, ApiServerKind::Central, &cfg(true));
		assert_eq!(
			names(&es),
			vec![
				"tamanu-central-tasks",
				"tamanu-frontend",
				"tamanu-facility",
				"tamanu-central-api",
				"tamanu-central-fhir-resolve",
				"tamanu-central-fhir-refresh",
			]
		);
	}

	#[test]
	fn frontend_named_instances() {
		let es = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg(false));
		let fe = es.iter().find(|e| e.name == "tamanu-frontend").unwrap();
		assert_eq!(fe.instances, Instances::Named(&["a", "b"]));
		assert_eq!(fe.instances.min_count(), 2);
	}

	#[test]
	fn api_min_count_two() {
		let es = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg(false));
		let api = es.iter().find(|e| e.name == "tamanu-facility-api").unwrap();
		assert_eq!(api.instances, Instances::NumericAtLeast(2));
		assert_eq!(api.instances.min_count(), 2);
	}

	#[test]
	fn admits_instance_numeric_only_takes_digits() {
		let n = Instances::NumericAtLeast(1);
		assert!(n.admits_instance(Some("1")));
		assert!(n.admits_instance(Some("42")));
		assert!(!n.admits_instance(Some("a")));
		assert!(n.admits_instance(None)); // pm2
	}

	#[test]
	fn admits_instance_named_matches_listed() {
		let n = Instances::Named(&["a", "b"]);
		assert!(n.admits_instance(Some("a")));
		assert!(n.admits_instance(Some("b")));
		assert!(!n.admits_instance(Some("c")));
	}

	#[test]
	fn admits_instance_single_rejects_atsuffix() {
		assert!(Instances::Single.admits_instance(None));
		assert!(!Instances::Single.admits_instance(Some("1")));
	}

	fn criticality_for(es: &[Expectation], name: &str) -> Criticality {
		es.iter()
			.find(|e| e.name == name)
			.unwrap_or_else(|| panic!("no expectation named {name}"))
			.criticality
	}

	#[test]
	fn api_and_frontend_are_critical() {
		let central = expected(Supervisor::Systemd, ApiServerKind::Central, &cfg(false));
		assert_eq!(
			criticality_for(&central, "tamanu-central-api"),
			Criticality::Critical
		);
		assert_eq!(
			criticality_for(&central, "tamanu-frontend"),
			Criticality::Critical
		);

		let facility_pm2 = expected(Supervisor::Pm2, ApiServerKind::Facility, &cfg(false));
		assert_eq!(
			criticality_for(&facility_pm2, "tamanu-api"),
			Criticality::Critical
		);
	}

	#[test]
	fn tasks_sync_fhir_are_background() {
		let central = expected(Supervisor::Systemd, ApiServerKind::Central, &cfg(true));
		assert_eq!(
			criticality_for(&central, "tamanu-central-tasks"),
			Criticality::Background
		);
		assert_eq!(
			criticality_for(&central, "tamanu-central-fhir-resolve"),
			Criticality::Background
		);
		assert_eq!(
			criticality_for(&central, "tamanu-central-fhir-refresh"),
			Criticality::Background
		);

		let facility = expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg(false));
		assert_eq!(
			criticality_for(&facility, "tamanu-facility-sync"),
			Criticality::Background
		);
	}

	fn exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Up,
			criticality: Criticality::Background,
		}
	}

	#[test]
	fn match_names_empty_returns_everything() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks"), exp("tamanu-sync")];
		let m = match_names(&es, &[]).unwrap();
		assert_eq!(m.len(), 3);
	}

	#[test]
	fn match_names_substring() {
		let es = [exp("tamanu-central-api"), exp("tamanu-central-tasks")];
		let m = match_names(&es, &["api"]).unwrap();
		assert_eq!(m.len(), 1);
		assert_eq!(m[0].name, "tamanu-central-api");
	}

	#[test]
	fn match_names_union() {
		let es = [
			exp("tamanu-central-api"),
			exp("tamanu-central-tasks"),
			exp("tamanu-central-fhir-resolve"),
		];
		let m = match_names(&es, &["api", "fhir"]).unwrap();
		assert_eq!(m.len(), 2);
	}

	#[test]
	fn match_names_zero_match_bails() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks")];
		let err = match_names(&es, &["nope"]).unwrap_err();
		let msg = format!("{err}");
		assert!(msg.contains("nope"));
		assert!(msg.contains("tamanu-api"));
	}

	#[test]
	fn match_names_partial_typo_still_bails() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks")];
		let err = match_names(&es, &["api", "nope"]).unwrap_err();
		assert!(format!("{err}").contains("nope"));
	}

	#[test]
	fn parse_systemd_unit_works() {
		assert_eq!(
			parse_systemd_unit("tamanu-facility-api@1.service"),
			Some(("tamanu-facility-api", Some("1")))
		);
		assert_eq!(
			parse_systemd_unit("tamanu-frontend@a"),
			Some(("tamanu-frontend", Some("a")))
		);
		assert_eq!(
			parse_systemd_unit("tamanu-tasks.service"),
			Some(("tamanu-tasks", None))
		);
		assert_eq!(parse_systemd_unit("caddy.service"), None);
	}
}
