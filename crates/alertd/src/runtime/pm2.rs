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

use std::collections::HashMap;

use async_trait::async_trait;
use bestool_tamanu::pm2::{self, PmProc, Source};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::OnceCell;

use super::{Compute, Duty, Service, ServiceFacts, ServiceId, ServiceRuntime, Unavailable};

/// A Tamanu deployment run by pm2 on the machine this process is on.
pub struct Pm2Runtime {
	/// The deployment's version, which every one of its processes runs.
	version: Option<String>,
	listing: OnceCell<Result<Listing, Unavailable>>,
}

/// What one pm2 listing produced, and whether its process states can be
/// believed.
#[derive(Clone)]
struct Listing {
	procs: Vec<PmProc>,
	/// Resident memory in bytes and cumulative processor seconds, by pid.
	///
	/// Taken for the whole listing at once. Enumerating processes is slowest on
	/// exactly the platform pm2 runs on, and three checks walk the workload each
	/// sweep, so building a `System` per service per check was the worst shape
	/// available.
	usage: HashMap<u32, (u64, f64)>,
	/// Why no service's facts can be read from this listing.
	///
	/// When the pm2 CLI is unreachable we fall back to reading `dump.pm2`, which
	/// names the processes but not their state: we then infer each one's from a
	/// pid file and the OS process table, both of which come back empty for a
	/// user that cannot see the processes. Nothing alive in that listing is the
	/// permissions symptom, not an application that is down, and reporting it as
	/// down would be a reading we did not take.
	indeterminate: Option<String>,
}

impl Pm2Runtime {
	/// `version` is the deployment's own, as the sweep resolved it; `None`
	/// where it could not be resolved, which leaves each service's version
	/// unknown rather than asserting one.
	pub fn new(version: Option<String>) -> Self {
		Self {
			version,
			listing: OnceCell::new(),
		}
	}

	async fn listing(&self) -> Result<&Listing, Unavailable> {
		self.listing
			.get_or_init(|| async {
				// `pm2::list` shells out to the pm2 Node CLI with a blocking
				// command, which on Windows (pm2.cmd → cmd.exe → node.exe, each
				// antivirus-scanned) takes over a second, and building the
				// listing walks the process table after it. Both off the
				// executor so they cannot stall the checks sharing it.
				let built = tokio::task::spawn_blocking(|| {
					pm2::list().map(|(procs, source)| {
						Listing::new(
							procs
								.into_iter()
								.filter(|p| p.name.starts_with("tamanu-"))
								.collect(),
							source,
						)
					})
				})
				.await;

				match built {
					Ok(Ok(listing)) => Ok(listing),
					Ok(Err(err)) => Err(Unavailable::new(format!(
						"could not list pm2 processes: {err}"
					))),
					Err(err) => Err(Unavailable::new(format!("pm2 listing task failed: {err}"))),
				}
			})
			.await
			.as_ref()
			.map_err(Clone::clone)
	}
}

impl Listing {
	fn new(procs: Vec<PmProc>, source: Source) -> Self {
		let indeterminate = (matches!(source, Source::Dump)
			&& !procs.is_empty()
			&& procs.iter().all(|p| !p.running))
		.then(|| {
			"read pm2's dump file but couldn't verify any process is alive — likely a permissions issue (try running elevated)"
				.to_string()
		});
		let usage = if indeterminate.is_some() {
			// Nothing will be asked for these; don't walk the process table for
			// a listing whose facts are refused anyway.
			HashMap::new()
		} else {
			usage_of(procs.iter().filter_map(|proc| proc.pid))
		};
		Self {
			procs,
			usage,
			indeterminate,
		}
	}
}

/// Resident memory in bytes and cumulative processor time in seconds, for every
/// pid given, in one pass over the process table.
///
/// A pid gone by the time we look is absent from the result rather than zero.
fn usage_of(pids: impl IntoIterator<Item = u32>) -> HashMap<u32, (u64, f64)> {
	let pids: Vec<Pid> = pids.into_iter().map(Pid::from_u32).collect();
	if pids.is_empty() {
		return HashMap::new();
	}

	let mut sys = System::new();
	sys.refresh_processes_specifics(
		ProcessesToUpdate::Some(&pids),
		false,
		ProcessRefreshKind::nothing().with_memory().with_cpu(),
	);
	pids.into_iter()
		.filter_map(|pid| {
			let proc = sys.process(pid)?;
			Some((
				pid.as_u32(),
				(proc.memory(), proc.accumulated_cpu_time() as f64 / 1000.0),
			))
		})
		.collect()
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
			.listing()
			.await?
			.procs
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
		let listing = self.listing().await?;
		if let Some(reason) = listing.indeterminate.as_deref() {
			return Err(Unavailable::new(reason));
		}
		let Some(proc) = listing.procs.iter().find(|p| service_id(p) == *id) else {
			return Err(Unavailable::new(format!("pm2 has no process {id}")));
		};

		let usage = proc.pid.and_then(|pid| listing.usage.get(&pid)).copied();

		Ok(ServiceFacts {
			up: proc.running,
			version: Ok(self.version.clone()),
			memory_bytes: usage.map(|(memory, _)| memory),
			// pm2 declares no ceiling for a process, so there is no denominator
			// to grade its memory against and only the metric is reported.
			memory_ceiling_bytes: None,
			processor_seconds: usage.map(|(_, processor)| processor),
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn proc(name: &str, pm_id: Option<i64>) -> PmProc {
		running_proc(name, pm_id, true)
	}

	fn running_proc(name: &str, pm_id: Option<i64>, running: bool) -> PmProc {
		PmProc {
			name: name.into(),
			pm_id,
			running,
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

	/// Falling back to `dump.pm2` loses the only authority on which processes
	/// are alive, so nothing verifiably alive there is a permissions symptom
	/// rather than an application that is down. The facts are unreadable, and
	/// the check reports that it could not tell rather than a shortfall.
	#[test]
	fn a_dump_listing_with_nothing_alive_cannot_be_read() {
		let listing = Listing::new(
			vec![
				running_proc("tamanu-api", Some(0), false),
				running_proc("tamanu-tasks", Some(1), false),
			],
			Source::Dump,
		);
		assert!(listing.indeterminate.is_some());
	}

	/// One process verifiably alive means the listing is being read, so the
	/// rest really are down.
	#[test]
	fn a_dump_listing_with_something_alive_is_believed() {
		let listing = Listing::new(
			vec![
				running_proc("tamanu-api", Some(0), true),
				running_proc("tamanu-tasks", Some(1), false),
			],
			Source::Dump,
		);
		assert!(listing.indeterminate.is_none());
	}

	/// The CLI is authoritative: everything down there is the truth.
	#[test]
	fn a_cli_listing_is_always_believed() {
		let listing = Listing::new(
			vec![running_proc("tamanu-api", Some(0), false)],
			Source::Cli,
		);
		assert!(listing.indeterminate.is_none());
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

	/// The process table is walked once for the listing, not once per service
	/// per check: enumerating processes is slowest on exactly the platform pm2
	/// runs on.
	#[test]
	fn usage_is_taken_for_the_whole_listing_at_once() {
		// This process is one we know is alive, so it stands in for a supervised
		// one without needing pm2 on the box.
		let me = std::process::id();
		let listing = Listing::new(
			vec![PmProc {
				name: "tamanu-api".into(),
				pm_id: Some(0),
				running: true,
				pid: Some(me),
			}],
			Source::Cli,
		);
		assert!(
			listing.usage.contains_key(&me),
			"the listing carries its own usage rather than fetching per service"
		);
	}

	/// A listing whose facts are refused anyway is not worth walking the process
	/// table for.
	#[test]
	fn an_indeterminate_listing_walks_nothing() {
		let listing = Listing::new(
			vec![PmProc {
				name: "tamanu-api".into(),
				pm_id: Some(0),
				running: false,
				pid: Some(std::process::id()),
			}],
			Source::Dump,
		);
		assert!(listing.indeterminate.is_some());
		assert!(listing.usage.is_empty());
	}
}
