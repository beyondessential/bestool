//! Thin async wrapper around the systemd manager D-Bus interface.
//!
//! Replaces ad-hoc subprocess calls to `systemctl` across the tamanu
//! commands. Operations that mutate units still need root (or polkit
//! authorisation) — the bus auth surface is the same as `systemctl`'s.
//!
//! On non-Linux platforms every function is a stub: reads return empty /
//! false, mutations bail. The tamanu lifecycle dispatcher only selects
//! `Supervisor::Systemd` on Linux, so the stubs never execute — they exist so
//! call sites compile across platforms without cfg gates.

use std::collections::HashSet;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(not(target_os = "linux"))]
pub use stub::*;

/// Snapshot of one entry from `ListUnitsByPatterns`.
///
/// Field names mirror the systemd D-Bus method's tuple positions — `name`
/// (unit), `load_state`, `active_state`, `sub_state`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitState {
	pub name: String,
	pub load_state: String,
	pub active_state: String,
	pub sub_state: String,
}

impl UnitState {
	/// True when systemd considers the unit running — `active=active` and a
	/// `sub_state` of `running` or `exited`. Matches the previous text-parsed
	/// definition in `lifecycle::discover_systemd`.
	pub fn running(&self) -> bool {
		self.active_state == "active" && (self.sub_state == "running" || self.sub_state == "exited")
	}
}

/// One entry from `ListUnitFilesByPatterns`: a unit that is installed on the
/// host, whether or not it is currently loaded.
///
/// Distinct from [`UnitState`], which only covers units systemd has loaded. A
/// unit can be installed and enabled without being loaded — the state an
/// operator leaves behind by enabling a unit they have not started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitFile {
	/// The unit name, e.g. `tamanu-central-api@1.service`. The D-Bus method
	/// answers with the unit file's path; this is its basename.
	pub name: String,
	/// `enabled`, `disabled`, `static`, `masked`, …
	pub state: String,
}

impl UnitFile {
	/// Whether systemd will start this unit of its own accord at boot.
	pub fn enabled(&self) -> bool {
		self.state == "enabled" || self.state == "enabled-runtime"
	}
}

/// The resource readings systemd holds for one service unit.
///
/// Every field is optional because systemd answers `u64::MAX` for a reading it
/// does not have — an unset `MemoryMax` is infinity, and a unit with no cgroup
/// has no `MemoryCurrent` — and infinity is not a number a caller should ever
/// see as a quantity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnitResources {
	/// `MemoryCurrent`, in bytes.
	pub memory_bytes: Option<u64>,
	/// `MemoryMax`, in bytes: the ceiling declared for this unit specifically.
	pub memory_max_bytes: Option<u64>,
	/// `CPUUsageNSec`: processor time consumed since the unit started.
	pub processor_nanos: Option<u64>,
}

/// Probe `is_enabled` for many unit names in one go and return the subset
/// that came back `enabled` or `enabled-runtime`. Errors on individual probes
/// are treated as "not enabled" — matches the previous best-effort semantics.
pub async fn collect_enabled<I, S>(units: I) -> HashSet<String>
where
	I: IntoIterator<Item = S>,
	S: Into<String>,
{
	let mut out = HashSet::new();
	for unit in units {
		let unit = unit.into();
		if is_enabled(&unit).await.unwrap_or(false) {
			out.insert(unit);
		}
	}
	out
}

#[cfg(target_os = "linux")]
mod linux {
	use futures::StreamExt;
	use miette::{IntoDiagnostic, Result, bail, miette};
	use tokio::sync::OnceCell;
	use tracing::debug;
	use zbus_systemd::{
		systemd1::{JobRemovedStream, ManagerProxy, ServiceProxy, UnitProxy},
		zbus::{self, Connection, zvariant::OwnedObjectPath},
	};

	use super::{UnitFile, UnitResources, UnitState};

	static CONNECTION: OnceCell<Connection> = OnceCell::const_new();

	/// How long any one read off the bus may take.
	///
	/// A read that never returns is worse than one that fails: the sweep these
	/// feed has to finish and report, and a check that hangs takes the whole
	/// sweep with it — so nothing reaches canopy and the watchdog restarts the
	/// daemon into the same hang. Every caller already handles a read that
	/// could not be taken, so a bounded failure degrades into a skip.
	///
	/// Generous against a healthy bus, where these answer in milliseconds.
	const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

	/// Bound a read off the bus, naming it if it runs out of time.
	async fn bounded<T>(
		what: &str,
		read: impl std::future::Future<Output = Result<T>>,
	) -> Result<T> {
		match tokio::time::timeout(READ_TIMEOUT, read).await {
			Ok(result) => result,
			Err(_) => bail!("systemd {what} did not answer within {READ_TIMEOUT:?}"),
		}
	}

	async fn connection() -> Result<&'static Connection> {
		CONNECTION
			.get_or_try_init(|| async {
				Connection::system()
					.await
					.into_diagnostic()
					.map_err(|e| e.wrap_err("opening system D-Bus connection"))
			})
			.await
	}

	async fn manager() -> Result<ManagerProxy<'static>> {
		ManagerProxy::new(connection().await?)
			.await
			.into_diagnostic()
	}

	/// `systemctl list-units ... <patterns>`. Empty `patterns` returns nothing.
	///
	/// Drops `load-state == not-found` entries to match the previous text path,
	/// which skipped them.
	pub async fn list_units(patterns: &[&str]) -> Result<Vec<UnitState>> {
		if patterns.is_empty() {
			return Ok(Vec::new());
		}
		bounded("list_units", list_units_inner(patterns)).await
	}

	async fn list_units_inner(patterns: &[&str]) -> Result<Vec<UnitState>> {
		let mgr = manager().await?;
		let raw = mgr
			.list_units_by_patterns(
				Vec::new(),
				patterns.iter().map(|s| (*s).to_string()).collect(),
			)
			.await
			.into_diagnostic()?;
		Ok(raw
			.into_iter()
			.filter(|u| u.2 != "not-found")
			.map(|u| UnitState {
				name: u.0,
				load_state: u.2,
				active_state: u.3,
				sub_state: u.4,
			})
			.collect())
	}

	/// `systemctl list-unit-files <patterns>`. Empty `patterns` returns nothing.
	///
	/// Covers units that are installed but not loaded, which
	/// [`list_units`] cannot see. The D-Bus method answers with each unit
	/// file's path; the name is its basename.
	pub async fn list_unit_files(patterns: &[&str]) -> Result<Vec<UnitFile>> {
		if patterns.is_empty() {
			return Ok(Vec::new());
		}
		bounded("list_unit_files", list_unit_files_inner(patterns)).await
	}

	async fn list_unit_files_inner(patterns: &[&str]) -> Result<Vec<UnitFile>> {
		let mgr = manager().await?;
		let raw = mgr
			.list_unit_files_by_patterns(
				Vec::new(),
				patterns.iter().map(|s| (*s).to_string()).collect(),
			)
			.await
			.into_diagnostic()?;
		Ok(raw
			.into_iter()
			.map(|(path, state)| UnitFile {
				name: path.rsplit('/').next().unwrap_or(&path).to_string(),
				state,
			})
			.collect())
	}

	/// Read `MemoryCurrent`, `MemoryMax` and `CPUUsageNSec` for a service unit.
	///
	/// `u64::MAX` is systemd's "no value" for each of these — an unset
	/// `MemoryMax` is infinity, and a unit with no cgroup (not running, or
	/// accounting off) has neither a current nor a usage — so it maps to
	/// `None` rather than being passed on as a quantity.
	pub async fn unit_resources(unit: &str) -> Result<UnitResources> {
		bounded("unit_resources", unit_resources_inner(unit)).await
	}

	async fn unit_resources_inner(unit: &str) -> Result<UnitResources> {
		let conn = connection().await?;
		let mgr = ManagerProxy::new(conn).await.into_diagnostic()?;
		let path = mgr
			.get_unit(unit.to_string())
			.await
			.map_err(|e| miette!("systemd get_unit({unit}) failed: {e}"))?;
		let service = ServiceProxy::builder(conn)
			.path(path)
			.into_diagnostic()?
			.build()
			.await
			.into_diagnostic()?;

		let finite = |v: Result<u64, zbus::Error>| v.ok().filter(|n| *n != u64::MAX);
		Ok(UnitResources {
			memory_bytes: finite(service.memory_current().await),
			memory_max_bytes: finite(service.memory_max().await),
			processor_nanos: finite(service.cpu_usage_n_sec().await),
		})
	}

	/// The service unit a process belongs to, by pid.
	///
	/// `None` when the pid is in no unit systemd owns. Only meaningful for a
	/// pid on this machine: a pid from elsewhere resolves against this host's
	/// process table and would name whatever happens to hold that number.
	pub async fn unit_for_pid(pid: u32) -> Result<Option<String>> {
		bounded("unit_for_pid", unit_for_pid_inner(pid)).await
	}

	async fn unit_for_pid_inner(pid: u32) -> Result<Option<String>> {
		let conn = connection().await?;
		let mgr = ManagerProxy::new(conn).await.into_diagnostic()?;
		let path = match mgr.get_unit_by_pid(pid).await {
			Ok(path) => path,
			Err(zbus::Error::MethodError(..)) => return Ok(None),
			Err(e) => return Err(miette!("systemd get_unit_by_pid({pid}) failed: {e}")),
		};
		let unit = UnitProxy::builder(conn)
			.path(path)
			.into_diagnostic()?
			.build()
			.await
			.into_diagnostic()?;
		unit.id()
			.await
			.map(Some)
			.map_err(|e| miette!("reading the unit id for pid {pid} failed: {e}"))
	}

	/// `systemctl is-active --quiet <unit>`. Returns true when the unit is
	/// currently `active`. Returns false for unknown / not-loaded units.
	pub async fn is_active(unit: &str) -> Result<bool> {
		bounded("is_active", is_active_inner(unit)).await
	}

	async fn is_active_inner(unit: &str) -> Result<bool> {
		let mgr = manager().await?;
		let raw = mgr
			.list_units_by_patterns(Vec::new(), vec![unit.to_string()])
			.await
			.into_diagnostic()?;
		Ok(raw.first().is_some_and(|u| u.3 == "active"))
	}

	/// True iff systemd has a unit file installed for `unit`. Mirrors
	/// `systemctl list-unit-files <unit>` returning a row. Useful for
	/// existence probes where the enabled/disabled state is irrelevant — e.g.
	/// "is the template `tamanu-patientportal@.service` installed at all?".
	pub async fn unit_file_exists(unit: &str) -> Result<bool> {
		bounded("unit_file_exists", unit_file_exists_inner(unit)).await
	}

	async fn unit_file_exists_inner(unit: &str) -> Result<bool> {
		let mgr = manager().await?;
		match mgr.get_unit_file_state(unit.to_string()).await {
			Ok(_) => Ok(true),
			Err(zbus::Error::MethodError(name, _, _))
				if matches!(
					name.as_str(),
					"org.freedesktop.systemd1.NoSuchUnit"
						| "org.freedesktop.systemd1.NoSuchUnitFile"
						| "org.freedesktop.DBus.Error.InvalidArgs"
				) =>
			{
				Ok(false)
			}
			Err(e) => Err(miette!("systemd get_unit_file_state({unit}) failed: {e}")),
		}
	}

	/// `systemctl is-enabled <unit>`. True for `enabled` and `enabled-runtime`,
	/// false for `disabled`, `static`, `masked`, `alias`, `linked`, `not-found`,
	/// and any not-loaded/not-installed errors.
	pub async fn is_enabled(unit: &str) -> Result<bool> {
		bounded("is_enabled", is_enabled_inner(unit)).await
	}

	async fn is_enabled_inner(unit: &str) -> Result<bool> {
		let mgr = manager().await?;
		match mgr.get_unit_file_state(unit.to_string()).await {
			Ok(state) => Ok(state == "enabled" || state == "enabled-runtime"),
			Err(zbus::Error::MethodError(name, _, _))
				if matches!(
					name.as_str(),
					"org.freedesktop.systemd1.NoSuchUnit"
						| "org.freedesktop.systemd1.NoSuchUnitFile"
						| "org.freedesktop.DBus.Error.InvalidArgs"
				) =>
			{
				Ok(false)
			}
			Err(e) => Err(miette!("systemd get_unit_file_state({unit}) failed: {e}")),
		}
	}

	/// `systemctl start <units>` with mode `replace`. Fires StartUnit per unit
	/// and returns once all jobs are enqueued — does not wait for completion.
	pub async fn start(units: &[String]) -> Result<()> {
		let mgr = manager().await?;
		for unit in units {
			mgr.start_unit(unit.clone(), "replace".into())
				.await
				.into_diagnostic()?;
		}
		Ok(())
	}

	/// `systemctl stop <units>` with mode `replace`. Fires StopUnit per unit and
	/// returns once all jobs are enqueued — does not wait for completion.
	pub async fn stop(units: &[String]) -> Result<()> {
		let mgr = manager().await?;
		for unit in units {
			mgr.stop_unit(unit.clone(), "replace".into())
				.await
				.into_diagnostic()?;
		}
		Ok(())
	}

	/// `systemctl disable <units>`. Persistent (not runtime-only). Empty input
	/// is a no-op.
	pub async fn disable(units: &[String]) -> Result<()> {
		if units.is_empty() {
			return Ok(());
		}
		let mgr = manager().await?;
		mgr.disable_unit_files(units.to_vec(), false)
			.await
			.into_diagnostic()?;
		Ok(())
	}

	/// `systemctl restart <unit>` with mode `replace`. Subscribes to
	/// `JobRemoved` before firing and awaits the matching signal, so the call
	/// returns only when systemd reports the job finished. A non-`done` job
	/// result bails.
	pub async fn restart(unit: &str) -> Result<()> {
		let mgr = manager().await?;
		let mut signals = mgr.receive_job_removed().await.into_diagnostic()?;
		let job = mgr
			.restart_unit(unit.into(), "replace".into())
			.await
			.into_diagnostic()?;
		wait_for_job(&mut signals, &job, "restart", unit).await
	}

	/// `systemctl restart <units>` with mode `replace`. Fires all restarts on
	/// one shared `JobRemoved` subscription, then waits for every job to come
	/// back `done`. Any non-`done` result bails. No-op for an empty slice.
	pub async fn restart_all(units: &[String]) -> Result<()> {
		if units.is_empty() {
			return Ok(());
		}
		let mgr = manager().await?;
		let mut signals = mgr.receive_job_removed().await.into_diagnostic()?;
		let mut pending: Vec<(OwnedObjectPath, &str)> = Vec::with_capacity(units.len());
		for unit in units {
			let job = mgr
				.restart_unit(unit.clone(), "replace".into())
				.await
				.into_diagnostic()?;
			pending.push((job, unit.as_str()));
		}
		while !pending.is_empty()
			&& let Some(removed) = signals.next().await
		{
			let args = removed.args().into_diagnostic()?;
			if let Some(idx) = pending.iter().position(|(j, _)| j == args.job()) {
				let (_, unit) = pending.remove(idx);
				let result = args.result();
				debug!(unit, verb = "restart", %result, "JobRemoved");
				if result != "done" {
					bail!("restart {unit}: job ended with result {result}");
				}
			}
		}
		if !pending.is_empty() {
			let names: Vec<&str> = pending.iter().map(|(_, u)| *u).collect();
			bail!(
				"JobRemoved stream closed before {} restart job(s) completed: {}",
				pending.len(),
				names.join(", ")
			);
		}
		Ok(())
	}

	/// `systemctl reload <unit>` with mode `replace`. Same JobRemoved-await
	/// semantics as `restart`.
	pub async fn reload(unit: &str) -> Result<()> {
		let mgr = manager().await?;
		let mut signals = mgr.receive_job_removed().await.into_diagnostic()?;
		let job = mgr
			.reload_unit(unit.into(), "replace".into())
			.await
			.into_diagnostic()?;
		wait_for_job(&mut signals, &job, "reload", unit).await
	}

	async fn wait_for_job(
		signals: &mut JobRemovedStream,
		job: &OwnedObjectPath,
		verb: &str,
		unit: &str,
	) -> Result<()> {
		while let Some(removed) = signals.next().await {
			let args = removed.args().into_diagnostic()?;
			if args.job() == job {
				let result = args.result();
				debug!(unit, verb, %result, "JobRemoved");
				if result == "done" {
					return Ok(());
				}
				bail!("{verb} {unit}: job ended with result {result}");
			}
		}
		bail!("{verb} {unit}: JobRemoved stream closed before job completed")
	}
}

#[cfg(not(target_os = "linux"))]
mod stub {
	use miette::{Result, bail};

	use super::{UnitFile, UnitResources, UnitState};

	const UNSUPPORTED: &str = "systemd is only available on Linux";

	pub async fn list_units(_: &[&str]) -> Result<Vec<UnitState>> {
		Ok(Vec::new())
	}
	pub async fn list_unit_files(_: &[&str]) -> Result<Vec<UnitFile>> {
		Ok(Vec::new())
	}
	pub async fn unit_resources(_: &str) -> Result<UnitResources> {
		Ok(UnitResources::default())
	}
	pub async fn unit_for_pid(_: u32) -> Result<Option<String>> {
		Ok(None)
	}
	pub async fn is_active(_: &str) -> Result<bool> {
		Ok(false)
	}
	pub async fn is_enabled(_: &str) -> Result<bool> {
		Ok(false)
	}
	pub async fn unit_file_exists(_: &str) -> Result<bool> {
		Ok(false)
	}
	pub async fn start(_: &[String]) -> Result<()> {
		bail!("{UNSUPPORTED}")
	}
	pub async fn stop(_: &[String]) -> Result<()> {
		bail!("{UNSUPPORTED}")
	}
	pub async fn disable(_: &[String]) -> Result<()> {
		bail!("{UNSUPPORTED}")
	}
	pub async fn restart(_: &str) -> Result<()> {
		bail!("{UNSUPPORTED}")
	}
	pub async fn restart_all(_: &[String]) -> Result<()> {
		bail!("{UNSUPPORTED}")
	}
	pub async fn reload(_: &str) -> Result<()> {
		bail!("{UNSUPPORTED}")
	}
}
