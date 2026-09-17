//! Reading a Tamanu deployment's services through systemd and the podman under
//! it.
//!
//! The two are one runtime rather than two: on a Linux deployment each service
//! is a podman container held by a systemd unit, so the unit is what lists and
//! names the service and the container is what its version comes from. Neither
//! alone answers the workload.
//!
//! spec: SUB

use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
};

use async_trait::async_trait;
use bestool_tamanu::{
	ApiServerKind,
	services::parse_systemd_unit,
	systemd::{self, UnitResources, UnitState},
	versions,
};
use tokio::sync::OnceCell;

use super::{Compute, Duty, Service, ServiceFacts, ServiceId, ServiceRuntime, Unavailable};

/// Units belonging to a Tamanu deployment. Every service the installer lays
/// down is named for the product it serves.
const UNIT_PATTERN: &str = "tamanu-*.service";

/// A Tamanu deployment run by systemd on the machine this process is on.
///
/// The listings are read once and shared: a sweep is a snapshot, and two checks
/// grading the same services must grade the same reading rather than two taken
/// moments apart.
pub struct SystemdRuntime {
	/// The role the deployment plays, which is what tells its units from those
	/// of a deployment of the other role on the same machine. A duty carries no
	/// role, so without this a leftover `tamanu-facility-api` would be reported
	/// as a central deployment's API service.
	kind: ApiServerKind,
	units: OnceCell<Result<Vec<UnitState>, Unavailable>>,
	unit_files: OnceCell<HashMap<String, bool>>,
	/// Unit name to the version of the container image it is running, as
	/// podman reports it.
	///
	/// An error here does not fail the listing: the workload is readable from
	/// systemd whether or not podman is, so only the version reading is
	/// unavailable and the check that grades drift is the only one that cares.
	versions: OnceCell<Result<HashMap<String, String>, Unavailable>>,
	/// One unit's resource readings, memoised like the listings above.
	///
	/// Reading them is a `GetUnit`, a fresh proxy and three property fetches,
	/// the first of which pulls the whole of systemd's service property set. The
	/// three checks that walk the workload would otherwise each pay that for
	/// every unit, and two of them only want the up-ness and version the
	/// listings above already hold.
	///
	/// A cell per unit rather than a map of values, so two checks asking for the
	/// same unit at once wait on one read instead of both issuing it.
	resources: Mutex<HashMap<String, Arc<OnceCell<UnitResources>>>>,
}

impl SystemdRuntime {
	pub fn new(kind: ApiServerKind) -> Self {
		Self {
			kind,
			units: OnceCell::new(),
			unit_files: OnceCell::new(),
			versions: OnceCell::new(),
			resources: Mutex::new(HashMap::new()),
		}
	}

	/// This unit's resource readings, read once for the life of the runtime.
	///
	/// A unit systemd would not answer for reads as nothing known rather than
	/// failing the facts: the up-ness and version beside them come from the
	/// listings and are still good.
	async fn resources(&self, unit: &str) -> UnitResources {
		let cell = {
			let mut cells = self.resources.lock().expect("unit resources lock");
			cells.entry(unit.to_string()).or_default().clone()
		};
		*cell
			.get_or_init(|| async {
				systemd::unit_resources(unit).await.unwrap_or_else(|err| {
					tracing::debug!(unit, %err, "could not read unit resources");
					Default::default()
				})
			})
			.await
	}

	async fn units(&self) -> Result<&[UnitState], Unavailable> {
		self.units
			.get_or_init(|| async {
				systemd::list_units(&[UNIT_PATTERN])
					.await
					.map_err(|err| Unavailable::new(format!("could not list systemd units: {err}")))
			})
			.await
			.as_deref()
			.map_err(Clone::clone)
	}

	/// Which units systemd will start of its own accord, by unit name.
	///
	/// A template instance is enabled either in its own right (a symlink for
	/// `tamanu-frontend@a.service`) or through its template, so both are
	/// recorded and the instance is looked up against both.
	async fn unit_files(&self) -> &HashMap<String, bool> {
		self.unit_files
			.get_or_init(|| async {
				match systemd::list_unit_files(&[UNIT_PATTERN]).await {
					Ok(files) => files
						.into_iter()
						.map(|file| (file.name.clone(), file.enabled()))
						.collect(),
					Err(err) => {
						tracing::debug!(%err, "could not list systemd unit files");
						HashMap::new()
					}
				}
			})
			.await
	}

	async fn versions(&self) -> Result<&HashMap<String, String>, Unavailable> {
		self.versions
			.get_or_init(|| async {
				versions::running_versions_linux().await.map_err(|err| {
					Unavailable::new(format!(
						"`podman ps` failed, so no container's version could be read: {err}"
					))
				})
			})
			.await
			.as_ref()
			.map_err(Clone::clone)
	}
}

/// Whether a unit belongs to a deployment of this role.
///
/// A unit carrying the other role's prefix is another application's, and must
/// not be reported as this one's. Everything else is this deployment's: the
/// role-neutral units (`tamanu-frontend`, `tamanu-patientportal`) are shared,
/// and the bare `tamanu-facility` singleton is the leftover both roles forbid.
fn belongs_to(kind: ApiServerKind, base: &str) -> bool {
	let other = match kind {
		ApiServerKind::Central => "tamanu-facility-",
		ApiServerKind::Facility => "tamanu-central-",
	};
	!base.starts_with(other)
}

/// Whether systemd will bring `unit` up on its own, given the installed unit
/// files. An instance inherits its template's enablement.
fn scheduled(unit_files: &HashMap<String, bool>, unit: &str, base: &str) -> bool {
	unit_files
		.get(unit)
		.copied()
		.or_else(|| unit_files.get(&format!("{base}@.service")).copied())
		.unwrap_or(false)
}

#[async_trait]
impl ServiceRuntime for SystemdRuntime {
	/// A machine-hosted deployment's compute is whatever the machine is doing,
	/// and this process is running on it. Switching an application's compute off
	/// while keeping its data is something a cluster does, not a supervisor.
	async fn compute(&self) -> Compute {
		Compute::Running
	}

	async fn services(&self) -> Result<Vec<Service>, Unavailable> {
		let units = self.units().await?;
		let unit_files = self.unit_files().await;

		let mut out = Vec::new();
		let mut seen = Vec::new();
		for unit in units {
			let Some((base, slot)) = parse_systemd_unit(&unit.name) else {
				continue;
			};
			if !belongs_to(self.kind, base) {
				continue;
			}
			seen.push(unit.name.clone());
			out.push(Service {
				id: ServiceId::new(&unit.name),
				duty: Duty::from_tamanu_service_name(base),
				slot: slot.map(str::to_string),
				scheduled: scheduled(unit_files, &unit.name, base),
			});
		}

		// A unit can be enabled without being loaded — an operator enabled it
		// and has not started it or rebooted since. It is part of the workload
		// the runtime intends to run, so it belongs in the listing with nothing
		// up; leaving it out would read as absent to a check that forbids it.
		for (name, enabled) in unit_files {
			if !enabled || seen.iter().any(|s| s == name) {
				continue;
			}
			// The template itself is not an instance of anything, so it names no
			// service. Its instances are listed in their own right.
			if name.contains("@.") {
				continue;
			}
			let Some((base, slot)) = parse_systemd_unit(name) else {
				continue;
			};
			if !belongs_to(self.kind, base) {
				continue;
			}
			out.push(Service {
				id: ServiceId::new(name),
				duty: Duty::from_tamanu_service_name(base),
				slot: slot.map(str::to_string),
				scheduled: true,
			});
		}

		Ok(out)
	}

	async fn service_facts(&self, id: &ServiceId) -> Result<ServiceFacts, Unavailable> {
		let units = self.units().await?;
		let up = units
			.iter()
			.find(|u| u.name == id.as_str())
			.is_some_and(UnitState::running);

		let resources = self.resources(id.as_str()).await;

		Ok(ServiceFacts {
			up,
			version: self
				.versions()
				.await
				.map(|versions| versions.get(id.as_str()).cloned()),
			memory_bytes: resources.memory_bytes,
			memory_ceiling_bytes: resources.memory_max_bytes,
			processor_seconds: resources
				.processor_nanos
				.map(|nanos| nanos as f64 / 1_000_000_000.0),
		})
	}
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;

	use super::*;

	fn files(entries: &[(&str, bool)]) -> HashMap<String, bool> {
		entries
			.iter()
			.map(|(name, on)| ((*name).to_string(), *on))
			.collect()
	}

	/// A template instance is enabled through its template, so an instance with
	/// no symlink of its own is still one systemd will bring up.
	#[test]
	fn an_instance_inherits_its_template_s_enablement() {
		let unit_files = files(&[("tamanu-frontend@.service", true)]);
		assert!(scheduled(
			&unit_files,
			"tamanu-frontend@a.service",
			"tamanu-frontend"
		));
	}

	/// An instance enabled in its own right counts even where the template is
	/// not — which is how a `.wants` symlink for one instance reads.
	#[test]
	fn an_instance_enabled_on_its_own_counts() {
		let unit_files = files(&[
			("tamanu-frontend@.service", false),
			("tamanu-frontend@a.service", true),
		]);
		assert!(scheduled(
			&unit_files,
			"tamanu-frontend@a.service",
			"tamanu-frontend"
		));
		assert!(!scheduled(
			&unit_files,
			"tamanu-frontend@b.service",
			"tamanu-frontend"
		));
	}

	/// Nothing known about a unit means nothing intends to run it.
	#[test]
	fn an_unknown_unit_is_not_scheduled() {
		assert!(!scheduled(
			&files(&[]),
			"tamanu-central-api@1.service",
			"tamanu-central-api"
		));
	}

	/// A duty carries no role, so it is the runtime that must not hand one
	/// deployment's services to the other's context.
	///
	/// spec: SUB#the-workload
	#[test]
	fn the_other_role_s_units_are_another_application_s() {
		assert!(belongs_to(ApiServerKind::Central, "tamanu-central-api"));
		assert!(!belongs_to(ApiServerKind::Central, "tamanu-facility-api"));
		assert!(belongs_to(ApiServerKind::Facility, "tamanu-facility-sync"));
		assert!(!belongs_to(ApiServerKind::Facility, "tamanu-central-tasks"));
	}

	/// The shared units belong to whichever deployment is asking, and so does
	/// the bare legacy singleton that both roles forbid.
	#[test]
	fn role_neutral_units_belong_to_both() {
		for kind in [ApiServerKind::Central, ApiServerKind::Facility] {
			assert!(belongs_to(kind, "tamanu-frontend"));
			assert!(belongs_to(kind, "tamanu-patientportal"));
			assert!(belongs_to(kind, "tamanu-facility"));
		}
	}

	/// Three checks walk the workload each sweep and two of them want only the
	/// up-ness and version the listings already hold, so a unit's resources are
	/// read once for the life of the runtime rather than once per ask.
	#[tokio::test]
	async fn a_unit_s_resources_are_read_once() {
		let runtime = SystemdRuntime::new(ApiServerKind::Central);
		let unit = "tamanu-central-api@1.service";

		// Concurrent asks for the same unit share one cell, so they wait on one
		// read rather than both issuing it.
		let (first, second) = tokio::join!(runtime.resources(unit), runtime.resources(unit));
		assert_eq!(first, second);

		let cells = runtime.resources.lock().expect("unit resources lock");
		assert_eq!(cells.len(), 1, "one cell for the one unit asked about");
		assert!(cells[unit].initialized());
	}
}
