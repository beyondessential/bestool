use clap::{Parser, Subcommand};
use miette::Result;
#[cfg(not(windows))]
use miette::miette;

use crate::actions::Context;

use super::RdpArgs;

#[cfg(windows)]
pub(crate) const SERVICE_NAME: &str = "bestool-rdp-monitor";
#[cfg(windows)]
const SERVICE_DISPLAY_NAME: &str = "BES RDP Monitor";
#[cfg(windows)]
const SERVICE_DESCRIPTION: &str = "Watches RDP sessions for fast user-switch (kick) events, writes a JSONL audit log, and raises a toast on the incoming session.";

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
		Err(miette!(
			"rdp service management is only available on Windows"
		))
	}
}

#[cfg(windows)]
pub use imp::dispatch_service_mode;

#[cfg(windows)]
mod imp {
	use std::{ffi::OsString, sync::Mutex, time::Duration};

	use miette::{IntoDiagnostic, Result, WrapErr, miette};
	use tokio::sync::watch;
	use tracing::{debug, info, warn};
	use windows_service::{
		define_windows_service,
		service::{
			ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl,
			ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
		},
		service_control_handler::{self, ServiceControlHandlerResult},
		service_dispatcher,
		service_manager::{ServiceManager, ServiceManagerAccess},
	};

	use super::{InstallArgs, SERVICE_DESCRIPTION, SERVICE_DISPLAY_NAME, SERVICE_NAME};
	use crate::actions::rdp::{
		audit::AuditLog,
		events::poll_events,
		monitor::{MonitorArgs, handle_event},
		state::Tracker,
	};

	const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

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

		let m = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
		let info = ServiceInfo {
			name: OsString::from(SERVICE_NAME),
			display_name: OsString::from(SERVICE_DISPLAY_NAME),
			service_type: SERVICE_TYPE,
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

	static SERVICE_ARGS: Mutex<Option<MonitorArgs>> = Mutex::new(None);

	define_windows_service!(ffi_service_main, service_main);

	fn service_main(_args: Vec<OsString>) {
		if let Err(err) = run_as_service() {
			warn!(?err, "service main exited with error");
		}
	}

	fn run_as_service() -> Result<()> {
		let args = SERVICE_ARGS
			.lock()
			.unwrap()
			.take()
			.ok_or_else(|| miette!("service args were not set before dispatch"))?;

		let (shutdown_tx, shutdown_rx) = watch::channel(false);
		let handler = move |event| -> ServiceControlHandlerResult {
			match event {
				ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
				ServiceControl::Stop | ServiceControl::Shutdown => {
					let _ = shutdown_tx.send(true);
					ServiceControlHandlerResult::NoError
				}
				_ => ServiceControlHandlerResult::NotImplemented,
			}
		};

		let status_handle = service_control_handler::register(SERVICE_NAME, handler)
			.into_diagnostic()
			.wrap_err("registering service control handler")?;

		status_handle
			.set_service_status(running_status())
			.into_diagnostic()?;

		let runtime = tokio::runtime::Builder::new_current_thread()
			.enable_all()
			.build()
			.into_diagnostic()
			.wrap_err("building service-mode tokio runtime")?;

		let result = runtime.block_on(service_loop(args, shutdown_rx));

		let exit = match &result {
			Ok(()) => ServiceExitCode::Win32(0),
			Err(_) => ServiceExitCode::Win32(1),
		};
		status_handle
			.set_service_status(stopped_status(exit))
			.into_diagnostic()?;

		result
	}

	async fn service_loop(args: MonitorArgs, mut shutdown: watch::Receiver<bool>) -> Result<()> {
		let mut audit = AuditLog::open(&args.audit_log)
			.await
			.wrap_err("opening audit log")?;
		let mut tracker = Tracker::new(Duration::from_secs(args.kick_window));
		let mut since =
			chrono::Utc::now() - chrono::Duration::seconds(args.poll_interval as i64);
		let mut last_record_id: u64 = 0;
		let mut interval = tokio::time::interval(Duration::from_secs(args.poll_interval));
		interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

		info!(service = SERVICE_NAME, "service monitor loop started");

		loop {
			tokio::select! {
				_ = interval.tick() => {
					let now = chrono::Utc::now();
					match poll_events(since).await {
						Ok(events) => {
							since = now;
							for ev in events {
								if ev.record_id <= last_record_id { continue; }
								last_record_id = ev.record_id;
								handle_event(ev, &mut tracker, &mut audit).await;
							}
						}
						Err(err) => warn!(?err, "failed to poll event log; will retry"),
					}
				}
				changed = shutdown.changed() => {
					changed.into_diagnostic().wrap_err("shutdown channel closed")?;
					if *shutdown.borrow() {
						debug!("shutdown signalled; exiting monitor loop");
						break;
					}
				}
			}
		}

		Ok(())
	}

	fn running_status() -> ServiceStatus {
		ServiceStatus {
			service_type: SERVICE_TYPE,
			current_state: ServiceState::Running,
			controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
			exit_code: ServiceExitCode::Win32(0),
			checkpoint: 0,
			wait_hint: Duration::default(),
			process_id: None,
		}
	}

	fn stopped_status(exit: ServiceExitCode) -> ServiceStatus {
		ServiceStatus {
			service_type: SERVICE_TYPE,
			current_state: ServiceState::Stopped,
			controls_accepted: ServiceControlAccept::empty(),
			exit_code: exit,
			checkpoint: 0,
			wait_hint: Duration::default(),
			process_id: None,
		}
	}

	/// Entry point invoked by `rdp monitor --service`. Blocks until the SCM
	/// signals the service to stop, or until the dispatcher returns an error.
	pub fn dispatch_service_mode(args: MonitorArgs) -> Result<()> {
		*SERVICE_ARGS.lock().unwrap() = Some(args);
		service_dispatcher::start(SERVICE_NAME, ffi_service_main)
			.into_diagnostic()
			.wrap_err("service dispatcher failed")
	}
}
