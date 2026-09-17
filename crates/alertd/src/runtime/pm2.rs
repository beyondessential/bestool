//! Reading a Tamanu deployment's services through pm2.
//!
//! The Windows deployment shape: every service is a Node process pm2 supervises
//! from one shared install, so there is no container and no per-service image.
//! Every process necessarily runs the install's version, which is a parameter
//! on the context rather than a reading, so a service's version here is the
//! deployment's.
//!
//! Resource usage comes from the OS process table rather than pm2's own `monit`
//! block: pm2 reports an instantaneous CPU percentage, where the OS reports the
//! cumulative time two sweeps apart can be turned into a rate.
//!
//! spec: SUB

use async_trait::async_trait;
use bestool_tamanu::pm2::{self, PmProc};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::OnceCell;

use super::{Compute, Duty, Service, ServiceFacts, ServiceId, ServiceRuntime, Unavailable};

/// A Tamanu deployment run by pm2 on the machine this process is on.
pub struct Pm2Runtime {
	/// The deployment's version, which every one of its processes runs.
	version: Option<String>,
	procs: OnceCell<Result<Vec<PmProc>, Unavailable>>,
}

impl Pm2Runtime {
	/// `version` is the deployment's own, as the sweep resolved it; `None`
	/// where it could not be resolved, which leaves each service's version
	/// unknown rather than asserting one.
	pub fn new(version: Option<String>) -> Self {
		Self {
			version,
			procs: OnceCell::new(),
		}
	}

	async fn procs(&self) -> Result<&[PmProc], Unavailable> {
		self.procs
			.get_or_init(|| async {
				// `pm2::list` shells out to the pm2 Node CLI with a blocking
				// command, which on Windows (pm2.cmd → cmd.exe → node.exe, each
				// antivirus-scanned) takes over a second. Off the executor so it
				// cannot stall the checks sharing it.
				match tokio::task::spawn_blocking(pm2::list).await {
					Ok(Ok((procs, _source))) => Ok(procs
						.into_iter()
						.filter(|p| p.name.starts_with("tamanu-"))
						.collect()),
					Ok(Err(err)) => Err(Unavailable::new(format!(
						"could not list pm2 processes: {err}"
					))),
					Err(err) => Err(Unavailable::new(format!("pm2 listing task failed: {err}"))),
				}
			})
			.await
			.as_deref()
			.map_err(Clone::clone)
	}
}

/// How one pm2 process is named back to us. The pm2 id distinguishes the
/// several processes a clustered service runs under one name.
fn service_id(proc: &PmProc) -> ServiceId {
	match proc.pm_id {
		Some(id) => ServiceId::new(format!("{}#{id}", proc.name)),
		None => ServiceId::new(&proc.name),
	}
}

#[async_trait]
impl ServiceRuntime for Pm2Runtime {
	/// As on any machine-hosted deployment: this process runs on the machine
	/// serving the application, so its compute is on.
	async fn compute(&self) -> Compute {
		Compute::Running
	}

	async fn services(&self) -> Result<Vec<Service>, Unavailable> {
		Ok(self
			.procs()
			.await?
			.iter()
			.map(|proc| Service {
				id: service_id(proc),
				duty: Duty::from_tamanu_service_name(&proc.name),
				// pm2 has no `@instance` notation: the processes of a clustered
				// service all share one name and occupy no named slot.
				slot: None,
				// pm2 restarts whatever is in its process list, so being listed
				// is what says it intends to run.
				scheduled: true,
			})
			.collect())
	}

	async fn service_facts(&self, id: &ServiceId) -> Result<ServiceFacts, Unavailable> {
		let procs = self.procs().await?;
		let Some(proc) = procs.iter().find(|p| service_id(p) == *id) else {
			return Err(Unavailable::new(format!("pm2 has no process {id}")));
		};

		let usage = match proc.pid {
			Some(pid) => tokio::task::spawn_blocking(move || process_usage(pid))
				.await
				.unwrap_or_default(),
			None => None,
		};

		Ok(ServiceFacts {
			up: proc.running,
			version: self.version.clone(),
			memory_bytes: usage.map(|(memory, _)| memory),
			// pm2 declares no ceiling for a process, so there is no denominator
			// to grade its memory against and only the metric is reported.
			memory_ceiling_bytes: None,
			processor_seconds: usage.map(|(_, processor)| processor),
		})
	}
}

/// Resident memory in bytes and cumulative processor time in seconds, for one
/// pid. `None` when the process is gone by the time we look.
fn process_usage(pid: u32) -> Option<(u64, f64)> {
	let mut sys = System::new();
	let pid = Pid::from_u32(pid);
	sys.refresh_processes_specifics(
		ProcessesToUpdate::Some(&[pid]),
		false,
		ProcessRefreshKind::nothing().with_memory().with_cpu(),
	);
	let proc = sys.process(pid)?;
	Some((proc.memory(), proc.accumulated_cpu_time() as f64 / 1000.0))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn proc(name: &str, pm_id: Option<i64>) -> PmProc {
		PmProc {
			name: name.into(),
			pm_id,
			running: true,
			pid: None,
		}
	}

	/// A clustered service runs several processes under one name, so the pm2 id
	/// is what tells two of them apart.
	#[test]
	fn clustered_processes_get_their_own_ids() {
		let a = service_id(&proc("tamanu-api", Some(0)));
		let b = service_id(&proc("tamanu-api", Some(1)));
		assert_ne!(a, b);
		assert_eq!(a.as_str(), "tamanu-api#0");
	}

	/// Without a pm2 id there is only the name to go on.
	#[test]
	fn a_process_with_no_pm2_id_is_named_by_its_name() {
		assert_eq!(
			service_id(&proc("tamanu-tasks", None)).as_str(),
			"tamanu-tasks"
		);
	}

	/// pm2 names a service the same job systemd does, so a check grading duties
	/// grades the same thing on Windows as on Linux.
	///
	/// spec: SUB#the-duty-vocabulary
	#[test]
	fn pm2_names_map_onto_the_vocabulary() {
		use super::super::TamanuDuty;

		assert_eq!(
			Duty::from_tamanu_service_name("tamanu-api"),
			Duty::Tamanu(TamanuDuty::Api)
		);
		assert_eq!(
			Duty::from_tamanu_service_name("tamanu-sync"),
			Duty::Tamanu(TamanuDuty::Sync)
		);
	}
}
