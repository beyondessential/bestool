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
//!   - `patientportal` lives at `tamanu-patientportal` (no kind prefix). On
//!     older deployments it's a singleton; newer ones (and rolling forward)
//!     use a frontend-style template with `@a` and `@b` instances. Which
//!     layout is installed is detected at runtime by
//!     [`systemd_patient_portal_instanced`].
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
	/// Why this expectation has its current shape — surfaced in doctor
	/// diagnostics so operators can see *why* a given service is expected up
	/// or down. Examples: `"always required"`, `"kind is facility"`,
	/// `"config integrations.fhir.worker.enabled is false"`,
	/// `"DB setting features.patientPortal is true"`.
	pub reason: String,
	/// True for expectations that exist solely to catch leftover state from
	/// older deployment shapes — e.g. the `tamanu-facility` singleton unit
	/// from before facility servers split into per-role templates. Renderers
	/// hide compliant `legacy` rows by default so the 90% of deployments
	/// that never had the leftover aren't paying attention cost for a row
	/// that will always read OK. Non-compliant legacy rows still surface
	/// (and fail the check) just like any other.
	pub legacy: bool,
	/// True for services Caddy reverse-proxies to by container hostname —
	/// frontend, API, patient portal. Whenever one of these is started or
	/// restarted its podman container gets a fresh netavark IP, but Caddy
	/// and systemd-resolved still cache the previous one; without a reload
	/// the next request hits a stale upstream. Lifecycle commands key off
	/// this flag to decide when to call `lifecycle::reload_caddy`.
	pub behind_caddy: bool,
}

impl Expectation {
	/// Whether `bestool tamanu restart` should roll this service one
	/// instance at a time (with a readiness probe between each) instead of
	/// bulk-restarting. The criterion is purely technical: rolling needs at
	/// least two instances to keep one available during the roll. Single
	/// instances bulk-restart regardless of how user-facing they are.
	pub fn rolling_restart(&self) -> bool {
		self.instances.min_count() >= 2
	}
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
	/// We couldn't resolve what this service should be doing — typically
	/// because the signal that drives the expectation (e.g. a Tamanu DB
	/// setting) was unreachable. Lifecycle commands leave Unknown
	/// services alone: neither start nor stop them. Doctor and status
	/// report the row without flagging a failure.
	Unknown,
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
			Instances::NumericAtLeast(n) => {
				(1..=*n).map(|i| format!("{base}@{i}.service")).collect()
			}
			Instances::Named(xs) => xs.iter().map(|s| format!("{base}@{s}.service")).collect(),
		}
	}

	/// Whether a discovered `instance` (the bit after `@`, or `None` if there
	/// was no `@` suffix) is one this pattern admits under `supervisor`.
	///
	/// The `None` case is supervisor-dependent. pm2 has no `@instance`
	/// notation, so every process of a clustered service carries
	/// `instance: None` — there a templated pattern (`NumericAtLeast` /
	/// `Named`) must admit `None`. On systemd a `None` means a bare singleton
	/// `.service` unit, which does **not** satisfy a templated expectation:
	/// admitting it would let a leftover singleton (e.g. an older
	/// `tamanu-patientportal.service` on a host since migrated to the `@a`/`@b`
	/// template) masquerade as a healthy instanced member, and would make
	/// `tamanu restart` try to roll the singleton unit itself.
	pub fn admits_instance(&self, supervisor: Supervisor, instance: Option<&str>) -> bool {
		match (self, instance) {
			(Instances::Single, None) => true,
			(Instances::Single, Some(_)) => false,
			(Instances::NumericAtLeast(_), None) => matches!(supervisor, Supervisor::Pm2),
			(Instances::NumericAtLeast(_), Some(s)) => s.chars().all(|c| c.is_ascii_digit()),
			(Instances::Named(_), None) => matches!(supervisor, Supervisor::Pm2),
			(Instances::Named(xs), Some(s)) => xs.contains(&s),
		}
	}
}

/// All services expected to exist (or be absent) for this deployment.
///
/// `config` is `Some` when the install's config files are available, and `None`
/// when only a `TAMANU_DATABASE_URL` is — in which case the one config-derived
/// expectation (the FHIR worker) is reported as Unknown rather than guessed.
///
/// `patient_portal_enabled` carries the `features.patientPortal` setting
/// from the central server's DB as a tri-state:
/// - `Some(true)` → portal expected Up
/// - `Some(false)` → portal expected Down (the deployment has opted out)
/// - `None` → expectation is Unknown (signal couldn't be resolved, usually
///   because the DB is unreachable). Lifecycle commands skip Unknown
///   services so a transient outage doesn't trigger spurious stops/starts.
///
/// The signal is sourced from the DB rather than `local.json5` because
/// Tamanu's central-server code reads the setting (not the unused
/// `patientPortal.portalUrl` config field) to decide whether to mount the
/// portal API.
///
/// `patient_portal_instanced` decides the patient portal's unit shape: when
/// true, expect a frontend-style `Named(["a", "b"])` template; when false,
/// expect the historical singleton. Callers should source this from
/// [`systemd_patient_portal_instanced`] on systemd hosts and pass `false`
/// otherwise.
pub fn expected(
	supervisor: Supervisor,
	kind: ApiServerKind,
	config: Option<&TamanuConfig>,
	patient_portal_enabled: Option<bool>,
	patient_portal_instanced: bool,
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
		reason: "always required".into(),
		legacy: false,
		behind_caddy: false,
	});

	if matches!(supervisor, Supervisor::Systemd) {
		out.push(Expectation {
			name: "tamanu-frontend",
			instances: Instances::Named(&["a", "b"]),
			state: ExpectedState::Up,
			reason: "always required on systemd".into(),
			legacy: false,
			behind_caddy: true,
		});
		out.push(Expectation {
			name: "tamanu-facility",
			instances: Instances::Single,
			state: ExpectedState::Down,
			// criticality is unused for Down; Background is the harmless default.
			reason: "legacy singleton unit must not be present".into(),
			legacy: true,
			behind_caddy: false,
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
		reason: "always required".into(),
		legacy: false,
		behind_caddy: true,
	});

	match kind {
		ApiServerKind::Central => {
			let (resolve, refresh) = match supervisor {
				Supervisor::Pm2 => ("tamanu-fhir-resolve", "tamanu-fhir-refresh"),
				Supervisor::Systemd => {
					("tamanu-central-fhir-resolve", "tamanu-central-fhir-refresh")
				}
			};
			// FHIR services are expected Up when the worker is enabled in
			// config, and explicitly Down when it isn't — that way the doctor
			// catches the case where a deployment leaves the worker units
			// running after `integrations.fhir.worker.enabled` is flipped off.
			let (fhir_state, fhir_reason) = match config.map(|c| c.fhir_worker_enabled()) {
				Some(true) => (
					ExpectedState::Up,
					"config integrations.fhir.worker.enabled is true".to_string(),
				),
				Some(false) => (
					ExpectedState::Down,
					"config integrations.fhir.worker.enabled is false".to_string(),
				),
				None => (
					ExpectedState::Unknown,
					"no Tamanu config available; cannot read integrations.fhir.worker.enabled"
						.to_string(),
				),
			};
			out.push(Expectation {
				name: resolve,
				instances: Instances::Single,
				state: fhir_state,
				reason: fhir_reason.clone(),
				legacy: false,
				behind_caddy: false,
			});
			out.push(Expectation {
				name: refresh,
				instances: Instances::Single,
				state: fhir_state,
				reason: fhir_reason,
				legacy: false,
				behind_caddy: false,
			});

			// Patient portal layout depends on what the host actually
			// ships. The frontend-style A/B template (new + rolling
			// forward) lets rolling restart keep one instance up while
			// the other swaps; older deployments still ship a singleton
			// and bulk-restart. Expected Up iff the Tamanu DB setting
			// `features.patientPortal` is true — if the flag is on but
			// nothing's running, that's an ops misalignment worth
			// flagging. When the DB is unreachable we can't decide,
			// so the expectation is Unknown and lifecycle commands
			// leave the portal alone.
			if matches!(supervisor, Supervisor::Systemd) {
				let (portal_state, portal_reason) = match patient_portal_enabled {
					Some(true) => (
						ExpectedState::Up,
						"DB setting features.patientPortal is true".to_string(),
					),
					Some(false) => (
						ExpectedState::Down,
						"DB setting features.patientPortal is false".to_string(),
					),
					None => (
						ExpectedState::Unknown,
						"DB unreachable, cannot read features.patientPortal".to_string(),
					),
				};
				let portal_instances = if patient_portal_instanced {
					Instances::Named(&["a", "b"])
				} else {
					Instances::Single
				};
				out.push(Expectation {
					name: "tamanu-patientportal",
					instances: portal_instances,
					state: portal_state,
					reason: portal_reason,
					legacy: false,
					behind_caddy: true,
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
				reason: "kind is facility".into(),
				legacy: false,
				behind_caddy: false,
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
/// When a name in `names` matches zero expectations, the behaviour depends on
/// `ignore_unmatched`:
/// - `false` (the default for interactive use): returns an error naming the
///   unmatched patterns and the available names (typo safety in multi-name
///   invocations).
/// - `true`: warns about the unmatched patterns and proceeds with whatever did
///   match (possibly nothing). Lets an automated caller send a fixed list of
///   services without knowing which are enabled on this particular host — e.g.
///   the patient portal (central-only) or the FHIR worker (config-gated).
pub fn match_names<'a>(
	expectations: &'a [Expectation],
	names: &[&str],
	ignore_unmatched: bool,
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
		if ignore_unmatched {
			tracing::warn!(
				unmatched = unmatched.join(", "),
				"ignoring service name(s) not in this deployment's expected set"
			);
		} else {
			let available: Vec<&str> = expectations.iter().map(|e| e.name).collect();
			bail!(
				"no service matches: {}; available names are: {}",
				unmatched.join(", "),
				available.join(", "),
			);
		}
	}

	let matched: Vec<&Expectation> = expectations
		.iter()
		.filter(|e| names.iter().any(|name| e.name.contains(name)))
		.collect();
	Ok(matched)
}

/// Whether the patient portal is deployed as a templated multi-instance unit
/// (`tamanu-patientportal@.service`). Older deployments still ship the
/// singleton `tamanu-patientportal.service`; we detect which is installed by
/// asking systemd for the template's unit file.
///
/// Returns `false` on any D-Bus error, on hosts where the template isn't
/// installed, or on non-Linux systems — callers map `false` to the singleton
/// layout, which is the historical default and a safe fallback when the
/// expectation is Down anyway.
pub async fn systemd_patient_portal_instanced() -> bool {
	crate::systemd::unit_file_exists("tamanu-patientportal@.service")
		.await
		.unwrap_or(false)
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
			"integrations": { "fhir": { "worker": { "enabled": fhir_worker } } },
		});
		serde_json::from_value(json).unwrap()
	}

	fn names(es: &[Expectation]) -> Vec<&str> {
		es.iter().map(|e| e.name).collect()
	}

	#[test]
	fn facility_pm2_no_fhir() {
		let es = expected(
			Supervisor::Pm2,
			ApiServerKind::Facility,
			Some(&cfg(false)),
			Some(false),
			false,
		);
		assert_eq!(
			names(&es),
			vec!["tamanu-tasks", "tamanu-api", "tamanu-sync"]
		);
		// no Down expectations on pm2
		assert!(es.iter().all(|e| e.state == ExpectedState::Up));
	}

	#[test]
	fn central_pm2_with_fhir_disabled_expects_fhir_units_down() {
		// With the FHIR worker disabled in config, we want the doctor to
		// alert if the corresponding services are still running — that's
		// usually a deploy that's flipped the toggle without taking the
		// units down. So the expectations exist, but with state Down.
		let es = expected(
			Supervisor::Pm2,
			ApiServerKind::Central,
			Some(&cfg(false)),
			Some(false),
			false,
		);
		assert_eq!(
			names(&es),
			vec![
				"tamanu-tasks",
				"tamanu-api",
				"tamanu-fhir-resolve",
				"tamanu-fhir-refresh",
			]
		);
		for n in ["tamanu-fhir-resolve", "tamanu-fhir-refresh"] {
			let e = es.iter().find(|e| e.name == n).unwrap();
			assert_eq!(
				e.state,
				ExpectedState::Down,
				"{n} should be expected Down when fhir.worker.enabled = false"
			);
		}
	}

	#[test]
	fn central_pm2_with_fhir_adds_resolve_and_refresh() {
		let es = expected(
			Supervisor::Pm2,
			ApiServerKind::Central,
			Some(&cfg(true)),
			Some(false),
			false,
		);
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
		let es = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg(false)),
			Some(false),
			false,
		);
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
		let es = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg(true)),
			Some(true),
			true,
		);
		assert_eq!(
			names(&es),
			vec![
				"tamanu-central-tasks",
				"tamanu-frontend",
				"tamanu-facility",
				"tamanu-central-api",
				"tamanu-central-fhir-resolve",
				"tamanu-central-fhir-refresh",
				"tamanu-patientportal",
			]
		);
	}

	#[test]
	fn central_systemd_expects_patient_portal_instanced_when_template_installed() {
		// `features.patientPortal = true` + the templated unit file is
		// installed → expect a frontend-style A/B template.
		let es = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg(false)),
			Some(true),
			true,
		);
		let pp = es
			.iter()
			.find(|e| e.name == "tamanu-patientportal")
			.expect("patient portal expectation should be present");
		assert_eq!(pp.state, ExpectedState::Up);
		assert_eq!(pp.instances, Instances::Named(&["a", "b"]));
	}

	#[test]
	fn central_systemd_expects_patient_portal_singleton_on_older_deployments() {
		// `features.patientPortal = true` but the templated unit file isn't
		// installed → fall back to the historical singleton layout.
		let es = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg(false)),
			Some(true),
			false,
		);
		let pp = es
			.iter()
			.find(|e| e.name == "tamanu-patientportal")
			.expect("patient portal expectation should be present");
		assert_eq!(pp.state, ExpectedState::Up);
		assert_eq!(pp.instances, Instances::Single);
	}

	#[test]
	fn central_systemd_marks_patient_portal_unknown_when_db_unreachable() {
		// `patient_portal_enabled = None` means "we couldn't read the DB
		// flag" — the expectation must be Unknown so lifecycle commands
		// leave the portal alone rather than guessing Down.
		let es = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg(false)),
			None,
			false,
		);
		let pp = es
			.iter()
			.find(|e| e.name == "tamanu-patientportal")
			.expect("portal expectation should still be emitted");
		assert_eq!(pp.state, ExpectedState::Unknown);
		assert!(
			pp.reason.contains("DB"),
			"reason should mention DB: {}",
			pp.reason
		);
	}

	#[test]
	fn central_systemd_expects_patient_portal_down_when_db_flag_off() {
		// `features.patientPortal = false` → Tamanu's portal API is unmounted.
		// If the container's still running we want to know (stale install),
		// so the expectation stays in the list but with state Down. The
		// detected layout still drives which units we check for absence.
		let es = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg(false)),
			Some(false),
			false,
		);
		let pp = es
			.iter()
			.find(|e| e.name == "tamanu-patientportal")
			.expect("patient portal expectation should still be listed (as Down)");
		assert_eq!(pp.state, ExpectedState::Down);
	}

	#[test]
	fn facility_systemd_does_not_include_patient_portal() {
		let es = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg(false)),
			Some(true),
			true,
		);
		assert!(es.iter().all(|e| e.name != "tamanu-patientportal"));
	}

	#[test]
	fn central_pm2_does_not_include_patient_portal() {
		// pm2 is Windows-only; tamanu-patientportal is a systemd-managed unit
		// in our Linux deployments, so the pm2 expectation list shouldn't
		// include it regardless of the DB flag.
		let es = expected(
			Supervisor::Pm2,
			ApiServerKind::Central,
			Some(&cfg(true)),
			Some(true),
			true,
		);
		assert!(es.iter().all(|e| e.name != "tamanu-patientportal"));
	}

	#[test]
	fn frontend_named_instances() {
		let es = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg(false)),
			Some(false),
			false,
		);
		let fe = es.iter().find(|e| e.name == "tamanu-frontend").unwrap();
		assert_eq!(fe.instances, Instances::Named(&["a", "b"]));
		assert_eq!(fe.instances.min_count(), 2);
	}

	#[test]
	fn api_min_count_two() {
		let es = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg(false)),
			Some(false),
			false,
		);
		let api = es.iter().find(|e| e.name == "tamanu-facility-api").unwrap();
		assert_eq!(api.instances, Instances::NumericAtLeast(2));
		assert_eq!(api.instances.min_count(), 2);
	}

	#[test]
	fn admits_instance_numeric_only_takes_digits() {
		let n = Instances::NumericAtLeast(1);
		assert!(n.admits_instance(Supervisor::Systemd, Some("1")));
		assert!(n.admits_instance(Supervisor::Systemd, Some("42")));
		assert!(!n.admits_instance(Supervisor::Systemd, Some("a")));
		// pm2 clusters share one name with no @suffix, so None is admitted;
		// on systemd a bare singleton must not satisfy a numeric template.
		assert!(n.admits_instance(Supervisor::Pm2, None));
		assert!(!n.admits_instance(Supervisor::Systemd, None));
	}

	#[test]
	fn admits_instance_named_matches_listed() {
		let n = Instances::Named(&["a", "b"]);
		assert!(n.admits_instance(Supervisor::Systemd, Some("a")));
		assert!(n.admits_instance(Supervisor::Systemd, Some("b")));
		assert!(!n.admits_instance(Supervisor::Systemd, Some("c")));
		// Same supervisor split as the numeric case: pm2 None admitted,
		// systemd singleton rejected.
		assert!(n.admits_instance(Supervisor::Pm2, None));
		assert!(!n.admits_instance(Supervisor::Systemd, None));
	}

	#[test]
	fn admits_instance_single_rejects_atsuffix() {
		assert!(Instances::Single.admits_instance(Supervisor::Systemd, None));
		assert!(!Instances::Single.admits_instance(Supervisor::Systemd, Some("1")));
	}

	fn rolling_for(es: &[Expectation], name: &str) -> bool {
		es.iter()
			.find(|e| e.name == name)
			.unwrap_or_else(|| panic!("no expectation named {name}"))
			.rolling_restart()
	}

	#[test]
	fn api_and_frontend_are_rolling_restartable() {
		// Both run as templated/clustered units; rolling restart can keep
		// one instance live while the other swaps.
		let central = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg(false)),
			Some(false),
			false,
		);
		assert!(rolling_for(&central, "tamanu-central-api"));
		assert!(rolling_for(&central, "tamanu-frontend"));

		let facility_pm2 = expected(
			Supervisor::Pm2,
			ApiServerKind::Facility,
			Some(&cfg(false)),
			Some(false),
			false,
		);
		assert!(rolling_for(&facility_pm2, "tamanu-api"));
	}

	#[test]
	fn instanced_patient_portal_is_rolling_restartable() {
		// When the A/B template is installed the portal has two instances,
		// so rolling restart can keep one up while the other swaps.
		let central = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg(false)),
			Some(true),
			true,
		);
		assert!(rolling_for(&central, "tamanu-patientportal"));
	}

	#[test]
	fn singleton_patient_portal_bulk_restarts() {
		// Older deployments still ship the singleton; with only one instance
		// rolling can't keep traffic up, so we bulk-restart.
		let central = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg(false)),
			Some(true),
			false,
		);
		assert!(!rolling_for(&central, "tamanu-patientportal"));
	}

	#[test]
	fn singletons_bulk_restart() {
		// Single-instance services bulk-restart: there's no second instance
		// to take traffic during a roll.
		let central = expected(
			Supervisor::Systemd,
			ApiServerKind::Central,
			Some(&cfg(true)),
			Some(true),
			true,
		);
		assert!(!rolling_for(&central, "tamanu-central-tasks"));
		assert!(!rolling_for(&central, "tamanu-central-fhir-resolve"));
		assert!(!rolling_for(&central, "tamanu-central-fhir-refresh"));

		let facility = expected(
			Supervisor::Systemd,
			ApiServerKind::Facility,
			Some(&cfg(false)),
			Some(false),
			false,
		);
		assert!(!rolling_for(&facility, "tamanu-facility-sync"));
	}

	fn exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Up,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
		}
	}

	#[test]
	fn match_names_empty_returns_everything() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks"), exp("tamanu-sync")];
		let m = match_names(&es, &[], false).unwrap();
		assert_eq!(m.len(), 3);
	}

	#[test]
	fn match_names_substring() {
		let es = [exp("tamanu-central-api"), exp("tamanu-central-tasks")];
		let m = match_names(&es, &["api"], false).unwrap();
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
		let m = match_names(&es, &["api", "fhir"], false).unwrap();
		assert_eq!(m.len(), 2);
	}

	#[test]
	fn match_names_zero_match_bails() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks")];
		let err = match_names(&es, &["nope"], false).unwrap_err();
		let msg = format!("{err}");
		assert!(msg.contains("nope"));
		assert!(msg.contains("tamanu-api"));
	}

	#[test]
	fn match_names_partial_typo_still_bails() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks")];
		let err = match_names(&es, &["api", "nope"], false).unwrap_err();
		assert!(format!("{err}").contains("nope"));
	}

	#[test]
	fn match_names_ignore_unmatched_keeps_matched_and_skips_rest() {
		// Automated callers send a fixed list; a name absent from this
		// deployment's set (e.g. the central-only patient portal on a facility)
		// is skipped rather than failing the whole invocation.
		let es = [exp("tamanu-frontend"), exp("tamanu-facility-api")];
		let m = match_names(&es, &["frontend", "patientportal"], true).unwrap();
		assert_eq!(m.len(), 1);
		assert_eq!(m[0].name, "tamanu-frontend");
	}

	#[test]
	fn match_names_ignore_unmatched_all_absent_returns_empty() {
		// Every requested name is absent → empty match (nothing to do), not the
		// "no names = everything" path.
		let es = [exp("tamanu-frontend"), exp("tamanu-facility-api")];
		let m = match_names(&es, &["patientportal"], true).unwrap();
		assert!(m.is_empty());
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
