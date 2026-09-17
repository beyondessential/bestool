//! Reading a Postgres installation's services on the machine running it.
//!
//! A cluster is an application in its own right, and on a machine it is run by
//! one service: a systemd unit on Linux, a native Windows service on Windows.
//! That is why a runtime is resolved per application rather than per machine —
//! a Windows box runs Tamanu under pm2 and Postgres as a Windows service, and
//! no single machine-wide supervisor describes both.
//!
//! Which unit runs a given cluster is answered by the server itself. Asking it
//! for the pid of the backend serving us and asking systemd which unit holds
//! that pid is exact, where matching a port against each candidate unit's
//! configuration is guesswork. It only holds for a cluster on this machine, so
//! a remote one resolves no unit and reports no ceiling.
//!
//! spec: SUB

use async_trait::async_trait;
use bestool_postgres::pool::PgPool;
use bestool_tamanu::systemd;
use tokio::sync::OnceCell;

use super::{
	Compute, Duty, PostgresDuty, Service, ServiceFacts, ServiceId, ServiceRuntime, Unavailable,
};

/// A Postgres installation on the machine this process is on.
pub struct PgRuntime {
	/// The pool for this cluster, when it could be reached at all. Used to ask
	/// the server which service is running it and whether it is in recovery.
	pool: Option<PgPool>,
	/// Whether the cluster answers on this machine. A pid from a server
	/// elsewhere means nothing against this host's process table, so the unit
	/// is only resolved for a local one.
	local: bool,
	/// The identifier the application is keyed by, used to name the service
	/// where no supervisor unit could be resolved.
	key: String,
	resolved: OnceCell<Resolved>,
}

/// What one round-trip to the server settles: which service runs it, and which
/// duty that service is doing.
#[derive(Clone, Debug, Default)]
struct Resolved {
	unit: Option<String>,
	duty: Option<PostgresDuty>,
}

impl PgRuntime {
	pub fn new(key: impl Into<String>, pool: Option<PgPool>, local: bool) -> Self {
		Self {
			pool,
			local,
			key: key.into(),
			resolved: OnceCell::new(),
		}
	}

	async fn resolve(&self) -> &Resolved {
		self.resolved
			.get_or_init(|| async {
				let Some(pool) = self.pool.as_ref() else {
					return Resolved::default();
				};
				let Ok(client) = pool.get().await else {
					return Resolved::default();
				};
				let row = match client
					.query_one("SELECT pg_backend_pid(), pg_is_in_recovery()", &[])
					.await
				{
					Ok(row) => row,
					Err(err) => {
						tracing::debug!(%err, "could not ask postgres about itself");
						return Resolved::default();
					}
				};

				let duty = match row.try_get::<_, bool>(1) {
					Ok(true) => Some(PostgresDuty::Replica),
					Ok(false) => Some(PostgresDuty::Primary),
					Err(_) => None,
				};

				let unit = match (self.local, row.try_get::<_, i32>(0)) {
					(true, Ok(pid)) if pid > 0 => match systemd::unit_for_pid(pid as u32).await {
						Ok(unit) => unit,
						Err(err) => {
							tracing::debug!(%err, "could not ask systemd which unit runs postgres");
							None
						}
					},
					_ => None,
				};

				Resolved { unit, duty }
			})
			.await
	}
}

#[async_trait]
impl ServiceRuntime for PgRuntime {
	/// A cluster on a machine has its compute on whenever the machine does, and
	/// this process is running on it.
	async fn compute(&self) -> Compute {
		Compute::Running
	}

	async fn services(&self) -> Result<Vec<Service>, Unavailable> {
		let resolved = self.resolve().await;
		Ok(vec![Service {
			id: ServiceId::new(resolved.unit.clone().unwrap_or_else(|| self.key.clone())),
			// A server that would not say whether it is in recovery is taken
			// for the primary, which is what a lone cluster on a machine is.
			duty: Duty::Postgres(resolved.duty.unwrap_or(PostgresDuty::Primary)),
			slot: None,
			scheduled: true,
		}])
	}

	async fn service_facts(&self, id: &ServiceId) -> Result<ServiceFacts, Unavailable> {
		let resolved = self.resolve().await;
		let expected = resolved.unit.as_deref().unwrap_or(&self.key);
		if id.as_str() != expected {
			return Err(Unavailable::new(format!(
				"this Postgres installation has no service {id}"
			)));
		}

		// The server answering at all is what says the service is up; the
		// supervisor's own view is a second-hand reading of the same thing.
		let up = self.pool.is_some() && resolved.duty.is_some();

		let resources = match resolved.unit.as_deref() {
			Some(unit) => systemd::unit_resources(unit).await.unwrap_or_else(|err| {
				tracing::debug!(unit, %err, "could not read unit resources");
				Default::default()
			}),
			None => Default::default(),
		};

		Ok(ServiceFacts {
			up,
			// The server version is already the application's `pgVersion` fact,
			// read by the `version` check from the server itself rather than
			// from whatever supervises it.
			version: None,
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
	use super::*;

	/// Without a reachable server there is nothing to ask, so the installation
	/// still lists the one service it is but reports nothing running.
	///
	/// spec: SUB
	#[tokio::test]
	async fn an_unreachable_cluster_lists_its_service_as_down() {
		let runtime = PgRuntime::new("host-postgres-5432", None, true);
		let services = runtime.services().await.expect("listing needs no server");
		assert_eq!(services.len(), 1);
		assert_eq!(services[0].duty, Duty::Postgres(PostgresDuty::Primary));
		assert_eq!(services[0].id.as_str(), "host-postgres-5432");

		let facts = runtime
			.service_facts(&services[0].id)
			.await
			.expect("the service it just listed");
		assert!(!facts.up);
		assert_eq!(facts.memory_ceiling_bytes, None);
	}

	/// A service this installation did not list is not one it can answer for.
	#[tokio::test]
	async fn facts_for_another_service_are_unavailable() {
		let runtime = PgRuntime::new("host-postgres-5432", None, true);
		assert!(
			runtime
				.service_facts(&ServiceId::new("tamanu-central-api@1.service"))
				.await
				.is_err()
		);
	}
}
