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
		win::transition(&service_name(target, config), win::Desired::Running).await
	}
	#[cfg(not(any(unix, windows)))]
	{
		let _ = (target, config);
		Ok(())
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
	use std::time::Duration;

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
