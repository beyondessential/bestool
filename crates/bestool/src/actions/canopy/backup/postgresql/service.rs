//! Stop and start the managed postgres server around a restore.
//!
//! Unix drives systemd's `postgresql@<version>-<cluster>` unit. Windows drives
//! the EDB installer's Windows service (`postgresql-x64-<version>` by default,
//! overridable via the config's `service_name`) through the Service Control
//! Manager, waiting for it to reach the requested state — otherwise the still
//! open file handles on the data directory make the swap fail with a sharing
//! violation.

use miette::Result;

use super::resolve::ResolvedCluster;
use crate::actions::canopy::backup::method::PostgresqlConfig;

/// Stop the cluster, waiting until it is fully down.
pub async fn stop(target: &ResolvedCluster, config: &PostgresqlConfig) -> Result<()> {
	#[cfg(unix)]
	{
		let _ = config;
		systemctl("stop", target).await
	}
	#[cfg(windows)]
	{
		win::transition(&service_name(target, config), win::Desired::Stopped).await
	}
	#[cfg(not(any(unix, windows)))]
	{
		let _ = (target, config);
		Ok(())
	}
}

/// Start the cluster, waiting until it is accepting the service's control.
pub async fn start(target: &ResolvedCluster, config: &PostgresqlConfig) -> Result<()> {
	#[cfg(unix)]
	{
		let _ = config;
		systemctl("start", target).await
	}
	#[cfg(windows)]
	{
		let name = service_name(target, config);
		// A versioned EDB service for a cluster that isn't the live one is often left
		// Disabled, which blocks starting it outright. Enable it (Automatic) first so
		// the restored cluster comes up and stays up across reboots.
		win::set_start_type(&name, win::StartType::Automatic).await?;
		win::transition(&name, win::Desired::Running).await
	}
	#[cfg(not(any(unix, windows)))]
	{
		let _ = (target, config);
		Ok(())
	}
}

/// Stop, and set to manual start, the postgres services for every *other* installed
/// major version — so a differently-versioned server can't hold the port or
/// auto-restart over the cluster being restored. Best-effort. Windows-only: Debian
/// clusters listen on distinct ports, so they don't contend.
pub async fn quiesce_other_versions(keep_major: &str) {
	#[cfg(windows)]
	{
		for version in super::resolve::installed_server_versions() {
			if version == keep_major {
				continue;
			}
			let name = format!("postgresql-x64-{version}");
			if let Err(err) = win::stop_and_set_manual(&name).await {
				tracing::warn!("could not quiesce the postgres service {name}: {err}");
			}
		}
	}
	#[cfg(not(windows))]
	{
		let _ = keep_major;
	}
}

/// The account the cluster's service runs as, when it needs granting access to
/// the restored files (Windows: the EDB service account, e.g.
/// `NT AUTHORITY\NetworkService`). `None` when there's nothing extra to grant —
/// off Windows, or when the service runs as LocalSystem (already `SYSTEM`).
pub async fn service_account(target: &ResolvedCluster, config: &PostgresqlConfig) -> Option<String> {
	#[cfg(windows)]
	{
		win::query_account(&service_name(target, config)).await
	}
	#[cfg(not(windows))]
	{
		let _ = (target, config);
		None
	}
}

#[cfg(unix)]
async fn systemctl(verb: &str, target: &ResolvedCluster) -> Result<()> {
	let unit = format!("postgresql@{}-{}", target.version, target.cluster);
	super::run_status("systemctl", &[verb, &unit]).await
}

/// The Windows service name: the configured override, else the EDB installer's
/// default `postgresql-x64-<version>`.
#[cfg(windows)]
fn service_name(target: &ResolvedCluster, config: &PostgresqlConfig) -> String {
	config
		.service_name
		.clone()
		.unwrap_or_else(|| format!("postgresql-x64-{}", target.version))
}

#[cfg(windows)]
mod win {
	use std::{process::Stdio, time::Duration};

	use miette::{IntoDiagnostic as _, Result, WrapErr as _, bail};
	use tracing::info;
	use windows_service::{
		service::{ServiceAccess, ServiceState},
		service_manager::{ServiceManager, ServiceManagerAccess},
	};

	/// The state to drive the service into.
	#[derive(Clone, Copy)]
	pub enum Desired {
		Stopped,
		Running,
	}

	/// A service's start type, as `sc config start=` expects it.
	#[derive(Clone, Copy)]
	pub enum StartType {
		Automatic,
		Manual,
	}

	impl StartType {
		fn sc_value(self) -> &'static str {
			match self {
				StartType::Automatic => "auto",
				StartType::Manual => "demand",
			}
		}
	}

	/// The account the service runs as, per its SCM config. `None` if the service
	/// is absent/unreadable or runs as LocalSystem (already `SYSTEM`, and a name
	/// `icacls` doesn't accept). Best-effort — a failure to read it just means no
	/// extra grant.
	pub async fn query_account(name: &str) -> Option<String> {
		let name = name.to_owned();
		tokio::task::spawn_blocking(move || {
			let manager =
				ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
			let service = manager.open_service(&name, ServiceAccess::QUERY_CONFIG).ok()?;
			let account = service.query_config().ok()?.account_name?;
			let account = account.to_string_lossy().into_owned();
			if account.eq_ignore_ascii_case("localsystem") {
				None
			} else {
				Some(account)
			}
		})
		.await
		.ok()
		.flatten()
	}

	/// Set a service's start type via `sc config` — the Service Control Manager
	/// crate can only rewrite the *whole* config (clobbering the binary path), so
	/// `sc` is the safe way to change just the start type.
	pub async fn set_start_type(name: &str, start: StartType) -> Result<()> {
		let mode = start.sc_value();
		// `sc config <name> start= <mode>` — the space after `start=` is required.
		let output = tokio::process::Command::new("sc")
			.args(["config", name, "start=", mode])
			.stdin(Stdio::null())
			.output()
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("running sc config for {name}"))?;
		if !output.status.success() {
			bail!(
				"sc config {name} start= {mode} failed: {}",
				String::from_utf8_lossy(&output.stdout).trim()
			);
		}
		Ok(())
	}

	/// Stop the service (if present/running) and set it to manual start.
	pub async fn stop_and_set_manual(name: &str) -> Result<()> {
		let _ = transition(name, Desired::Stopped).await; // best-effort; may be absent/stopped
		set_start_type(name, StartType::Manual).await
	}

	/// Drive `name` to `desired`, waiting for the transition to settle. The SCM
	/// calls block, so run them off the async runtime.
	pub async fn transition(name: &str, desired: Desired) -> Result<()> {
		let name = name.to_owned();
		tokio::task::spawn_blocking(move || transition_blocking(&name, desired))
			.await
			.into_diagnostic()
			.wrap_err("joining service-control task")?
	}

	fn transition_blocking(name: &str, desired: Desired) -> Result<()> {
		let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
			.into_diagnostic()
			.wrap_err("connecting to the Service Control Manager")?;
		let service = manager
			.open_service(
				name,
				ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP,
			)
			.into_diagnostic()
			.wrap_err_with(|| format!("opening the postgres service {name:?}"))?;

		let current = service.query_status().into_diagnostic()?.current_state;
		match desired {
			Desired::Stopped => {
				if current == ServiceState::Stopped {
					return Ok(());
				}
				service
					.stop()
					.into_diagnostic()
					.wrap_err_with(|| format!("stopping the postgres service {name}"))?;
				wait_for(&service, ServiceState::Stopped, name)?;
				info!("stopped the postgres service {name}");
			}
			Desired::Running => {
				if current == ServiceState::Running {
					return Ok(());
				}
				service
					.start::<&str>(&[])
					.into_diagnostic()
					.wrap_err_with(|| format!("starting the postgres service {name}"))?;
				wait_for(&service, ServiceState::Running, name)?;
				info!("started the postgres service {name}");
			}
		}
		Ok(())
	}

	/// Poll the service until it reaches `want`. SCM transitions aren't atomic;
	/// give a stop or start up to a minute to complete.
	fn wait_for(
		service: &windows_service::service::Service,
		want: ServiceState,
		name: &str,
	) -> Result<()> {
		for _ in 0..120 {
			if service.query_status().into_diagnostic()?.current_state == want {
				return Ok(());
			}
			std::thread::sleep(Duration::from_millis(500));
		}
		bail!("the postgres service {name} did not reach {want:?} within 60s");
	}
}

#[cfg(all(test, windows))]
mod tests {
	use super::*;

	fn cluster(version: &str) -> ResolvedCluster {
		ResolvedCluster {
			data_dir: format!(r"C:\Program Files\PostgreSQL\{version}\data").into(),
			version: version.to_owned(),
			cluster: "main".to_owned(),
		}
	}

	fn config(service_name: Option<&str>) -> PostgresqlConfig {
		PostgresqlConfig {
			cluster: "main".into(),
			data_dir: None,
			version: None,
			connection_url: None,
			port: None,
			socket: None,
			strategy: None,
			service_name: service_name.map(str::to_owned),
		}
	}

	#[test]
	fn defaults_to_the_edb_service_name() {
		assert_eq!(service_name(&cluster("18"), &config(None)), "postgresql-x64-18");
	}

	#[test]
	fn the_override_wins() {
		assert_eq!(service_name(&cluster("18"), &config(Some("pg-custom"))), "pg-custom");
	}
}
