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
		systemd1::{JobRemovedStream, ManagerProxy},
		zbus::{self, Connection, zvariant::OwnedObjectPath},
	};

	use super::UnitState;

	static CONNECTION: OnceCell<Connection> = OnceCell::const_new();

	async fn manager() -> Result<ManagerProxy<'static>> {
		let conn = CONNECTION
			.get_or_try_init(|| async {
				Connection::system()
					.await
					.into_diagnostic()
					.map_err(|e| e.wrap_err("opening system D-Bus connection"))
			})
			.await?;
		ManagerProxy::new(conn).await.into_diagnostic()
	}

	/// `systemctl list-units ... <patterns>`. Empty `patterns` returns nothing.
	///
	/// Drops `load-state == not-found` entries to match the previous text path,
	/// which skipped them.
	pub async fn list_units(patterns: &[&str]) -> Result<Vec<UnitState>> {
		if patterns.is_empty() {
			return Ok(Vec::new());
		}
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

	/// `systemctl is-active --quiet <unit>`. Returns true when the unit is
	/// currently `active`. Returns false for unknown / not-loaded units.
	pub async fn is_active(unit: &str) -> Result<bool> {
		let mgr = manager().await?;
		let raw = mgr
			.list_units_by_patterns(Vec::new(), vec![unit.to_string()])
			.await
			.into_diagnostic()?;
		Ok(raw.first().is_some_and(|u| u.3 == "active"))
	}

	/// `systemctl is-enabled <unit>`. True for `enabled` and `enabled-runtime`,
	/// false for `disabled`, `static`, `masked`, `alias`, `linked`, `not-found`,
	/// and any not-loaded/not-installed errors.
	pub async fn is_enabled(unit: &str) -> Result<bool> {
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

	use super::UnitState;

	const UNSUPPORTED: &str = "systemd is only available on Linux";

	pub async fn list_units(_: &[&str]) -> Result<Vec<UnitState>> {
		Ok(Vec::new())
	}
	pub async fn is_active(_: &str) -> Result<bool> {
		Ok(false)
	}
	pub async fn is_enabled(_: &str) -> Result<bool> {
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
