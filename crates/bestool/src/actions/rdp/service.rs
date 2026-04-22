use clap::{Parser, Subcommand};
use miette::Result;
#[cfg(not(windows))]
use miette::miette;

use crate::actions::Context;

use super::RdpArgs;

#[cfg(windows)]
pub(crate) const SERVICE_NAME: &str = "bestool-rdp-monitor";
#[cfg(windows)]
pub(crate) const SERVICE_DISPLAY_NAME: &str = "BES RDP Monitor";
#[cfg(windows)]
pub(crate) const SERVICE_DESCRIPTION: &str =
	"Watches RDP sessions for fast user-switch (kick) events, writes a JSONL audit log, and raises a toast on the incoming session.";

/// Install, remove, start, stop, or query the `bestool-rdp-monitor` Windows
/// Service. All subcommands except `status` require Administrator rights.
#[derive(Debug, Clone, Parser)]
pub struct ServiceArgs {
	#[command(subcommand)]
	pub action: Action,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Action {
	/// Register the service with the Service Control Manager (auto-start).
	Install(InstallArgs),
	/// Remove the service from the Service Control Manager.
	Uninstall,
	/// Start the installed service.
	Start,
	/// Stop the running service.
	Stop,
	/// Print the current service state.
	Status,
}

/// Arguments forwarded to `bestool rdp monitor` each time the service starts.
/// Install-time snapshot: change these by uninstalling and re-installing.
#[derive(Debug, Clone, Parser)]
pub struct InstallArgs {
	/// Path to append-only JSONL audit log of every RDP session event.
	#[arg(long)]
	pub audit_log: Option<std::path::PathBuf>,

	/// Seconds between event log polls.
	#[arg(long)]
	pub poll_interval: Option<u64>,

	/// Max seconds between a disconnect and a new logon to count as a "kick".
	#[arg(long)]
	pub kick_window: Option<u64>,

	/// Only consider Tailscale source IPs for kick detection.
	#[arg(long)]
	pub tailscale_only: bool,
}

pub async fn run(ctx: Context<RdpArgs, ServiceArgs>) -> Result<()> {
	#[cfg(windows)]
	{
		match ctx.args_sub.action {
			Action::Install(args) => imp::install(args),
			Action::Uninstall => imp::uninstall(),
			Action::Start => imp::start(),
			Action::Stop => imp::stop(),
			Action::Status => imp::status(),
		}
	}

	#[cfg(not(windows))]
	{
		let _ = ctx;
		Err(miette!("rdp service management is only available on Windows"))
	}
}

#[cfg(windows)]
mod imp {
	use std::ffi::OsString;

	use miette::{IntoDiagnostic, Result, WrapErr, miette};
	use tracing::info;
	use windows_service::{
		service::{
			ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
			ServiceType,
		},
		service_manager::{ServiceManager, ServiceManagerAccess},
	};

	use super::{InstallArgs, SERVICE_DESCRIPTION, SERVICE_DISPLAY_NAME, SERVICE_NAME};

	fn require_admin() -> Result<()> {
		if !privilege::user::privileged() {
			return Err(miette!(
				"this command requires Administrator privileges (open an elevated shell)"
			));
		}
		Ok(())
	}

	fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
		ServiceManager::local_computer(None::<&str>, access)
			.into_diagnostic()
			.wrap_err("connecting to the Service Control Manager")
	}

	pub fn install(args: InstallArgs) -> Result<()> {
		require_admin()?;

		let exe = std::env::current_exe()
			.into_diagnostic()
			.wrap_err("resolving current exe path")?;

		let mut launch: Vec<OsString> = vec!["rdp".into(), "monitor".into(), "--service".into()];
		if let Some(p) = &args.audit_log {
			launch.push("--audit-log".into());
			launch.push(p.as_os_str().to_owned());
		}
		if let Some(n) = args.poll_interval {
			launch.push("--poll-interval".into());
			launch.push(n.to_string().into());
		}
		if let Some(n) = args.kick_window {
			launch.push("--kick-window".into());
			launch.push(n.to_string().into());
		}
		if args.tailscale_only {
			launch.push("--tailscale-only".into());
		}

		let m = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
		let info = ServiceInfo {
			name: OsString::from(SERVICE_NAME),
			display_name: OsString::from(SERVICE_DISPLAY_NAME),
			service_type: ServiceType::OWN_PROCESS,
			start_type: ServiceStartType::AutoStart,
			error_control: ServiceErrorControl::Normal,
			executable_path: exe,
			launch_arguments: launch,
			dependencies: vec![],
			account_name: None,
			account_password: None,
		};
		let svc = m
			.create_service(&info, ServiceAccess::CHANGE_CONFIG)
			.into_diagnostic()
			.wrap_err("creating service")?;
		svc.set_description(SERVICE_DESCRIPTION)
			.into_diagnostic()
			.wrap_err("setting service description")?;

		info!(service = SERVICE_NAME, "installed");
		Ok(())
	}

	pub fn uninstall() -> Result<()> {
		require_admin()?;
		let m = manager(ServiceManagerAccess::CONNECT)?;
		let svc = m
			.open_service(SERVICE_NAME, ServiceAccess::DELETE | ServiceAccess::STOP)
			.into_diagnostic()
			.wrap_err("opening service")?;
		let _ = svc.stop();
		svc.delete()
			.into_diagnostic()
			.wrap_err("deleting service")?;
		info!(service = SERVICE_NAME, "uninstalled");
		Ok(())
	}

	pub fn start() -> Result<()> {
		require_admin()?;
		let m = manager(ServiceManagerAccess::CONNECT)?;
		let svc = m
			.open_service(SERVICE_NAME, ServiceAccess::START)
			.into_diagnostic()
			.wrap_err("opening service")?;
		svc.start::<&str>(&[])
			.into_diagnostic()
			.wrap_err("starting service")?;
		info!(service = SERVICE_NAME, "start requested");
		Ok(())
	}

	pub fn stop() -> Result<()> {
		require_admin()?;
		let m = manager(ServiceManagerAccess::CONNECT)?;
		let svc = m
			.open_service(SERVICE_NAME, ServiceAccess::STOP)
			.into_diagnostic()
			.wrap_err("opening service")?;
		let status = svc
			.stop()
			.into_diagnostic()
			.wrap_err("stopping service")?;
		info!(service = SERVICE_NAME, state = ?status.current_state, "stop requested");
		Ok(())
	}

	pub fn status() -> Result<()> {
		let m = manager(ServiceManagerAccess::CONNECT)?;
		let svc = m
			.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
			.into_diagnostic()
			.wrap_err("opening service")?;
		let status = svc
			.query_status()
			.into_diagnostic()
			.wrap_err("querying status")?;
		let state = match status.current_state {
			ServiceState::Stopped => "stopped",
			ServiceState::StartPending => "start-pending",
			ServiceState::StopPending => "stop-pending",
			ServiceState::Running => "running",
			ServiceState::ContinuePending => "continue-pending",
			ServiceState::PausePending => "pause-pending",
			ServiceState::Paused => "paused",
		};
		println!("{SERVICE_NAME}: {state}");
		Ok(())
	}

}
