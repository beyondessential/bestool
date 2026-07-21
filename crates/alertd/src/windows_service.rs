//! Windows service integration for alertd daemon.
//!
//! This module provides native Windows service support, allowing alertd to be installed,
//! managed, and run as a Windows service through the Service Control Manager (SCM).
//!
//! The service integrates with Windows shutdown signals and properly reports its status
//! to the SCM throughout its lifecycle.

use std::{
	ffi::{OsStr, OsString},
	process::Command,
	sync::{Arc, Mutex},
	time::Duration,
};

use miette::{IntoDiagnostic, Result, bail, miette};
use tracing::{error, info};
use windows_service::{
	define_windows_service,
	service::{
		ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
		ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
		ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
	},
	service_control_handler::{self, ServiceControlHandlerResult},
	service_dispatcher,
	service_manager::{ServiceManager, ServiceManagerAccess},
};

use crate::DaemonConfig;

const SERVICE_NAME: &str = "bestool-alertd";
const SERVICE_DISPLAY_NAME: &str = "BES Alert Daemon";
const SERVICE_DESCRIPTION: &str =
	"Monitors and executes alert definitions from configuration files";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

/// Global storage for daemon configuration.
///
/// Required because the Windows service dispatcher calls service_main with only
/// command line arguments, so we store the config here before dispatching.
static SERVICE_CONFIG: Mutex<Option<DaemonConfig>> = Mutex::new(None);

define_windows_service!(ffi_service_main, service_main);

/// Runs the alertd daemon as a Windows service.
///
/// This function should be called when the executable is invoked by the Windows
/// Service Control Manager. It stores the configuration and dispatches to the
/// service main function.
///
/// # Errors
///
/// Returns an error if the service dispatcher fails to start or if the daemon
/// encounters a fatal error during execution.
pub fn run_service(config: DaemonConfig) -> Result<()> {
	// Store config in static so service_main can access it
	{
		let mut guard = SERVICE_CONFIG.lock().unwrap();
		*guard = Some(config);
	}

	service_dispatcher::start(SERVICE_NAME, ffi_service_main).into_diagnostic()?;
	Ok(())
}

/// Service entry point called by Windows Service Control Manager.
///
/// This is the FFI-safe entry point defined by `define_windows_service!` macro.
fn service_main(_arguments: Vec<OsString>) {
	if let Err(e) = run_service_main() {
		error!("service main error: {e:?}");
	}
}

/// Main service logic that manages the daemon lifecycle.
///
/// This function:
/// 1. Retrieves the daemon configuration from global storage
/// 2. Sets up a control handler for Windows service events (stop, shutdown)
/// 3. Reports service status to Windows SCM
/// 4. Runs the daemon with shutdown signal integration
/// 5. Handles graceful shutdown when requested by Windows
///
/// # Errors
///
/// Returns an error if the daemon fails to start or encounters a fatal error.
fn run_service_main() -> Result<()> {
	let config = {
		let mut guard = SERVICE_CONFIG.lock().unwrap();
		guard
			.take()
			.ok_or_else(|| miette::miette!("service config not set"))?
	};

	// Create shutdown channel for communicating with daemon
	let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
	let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));
	let shutdown_tx_clone = shutdown_tx.clone();

	// Event handler receives control events from Windows SCM
	let event_handler = move |control_event| -> ServiceControlHandlerResult {
		match control_event {
			ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
			ServiceControl::Stop | ServiceControl::Shutdown => {
				info!("received service stop/shutdown signal");
				// Signal daemon to shutdown gracefully
				let mut tx_guard = shutdown_tx_clone.lock().unwrap();
				if let Some(tx) = tx_guard.take() {
					let _ = tx.send(());
				}
				ServiceControlHandlerResult::NoError
			}
			_ => ServiceControlHandlerResult::NotImplemented,
		}
	};

	let status_handle =
		service_control_handler::register(SERVICE_NAME, event_handler).into_diagnostic()?;

	// Tell Windows that we're starting
	status_handle
		.set_service_status(ServiceStatus {
			service_type: SERVICE_TYPE,
			current_state: ServiceState::StartPending,
			controls_accepted: ServiceControlAccept::empty(),
			exit_code: ServiceExitCode::Win32(0),
			checkpoint: 1,
			wait_hint: Duration::from_secs(10),
			process_id: None,
		})
		.into_diagnostic()?;

	// Start the daemon in a new tokio runtime
	let runtime = tokio::runtime::Runtime::new().into_diagnostic()?;

	// Run the daemon (which handles its own startup)
	let result = runtime.block_on(async move {
		// Send periodic status updates while daemon is starting
		let status_tx = status_handle.clone();
		let status_task = tokio::spawn(async move {
			let mut checkpoint = 2;
			let mut is_running_reported = false;
			loop {
				// After first checkpoint, report as running
				if !is_running_reported && checkpoint > 2 {
					let _ = status_tx.set_service_status(ServiceStatus {
						service_type: SERVICE_TYPE,
						current_state: ServiceState::Running,
						controls_accepted: ServiceControlAccept::STOP
							| ServiceControlAccept::SHUTDOWN,
						exit_code: ServiceExitCode::Win32(0),
						checkpoint: 0,
						wait_hint: Duration::default(),
						process_id: None,
					});
					is_running_reported = true;
					info!("service reported as Running to Windows SCM");
				}

				tokio::time::sleep(Duration::from_secs(5)).await;

				// If already running, just send interrogate response
				if is_running_reported {
					continue;
				}

				// Still starting, send checkpoint updates to keep Windows from timing out
				let _ = status_tx.set_service_status(ServiceStatus {
					service_type: SERVICE_TYPE,
					current_state: ServiceState::StartPending,
					controls_accepted: ServiceControlAccept::empty(),
					exit_code: ServiceExitCode::Win32(0),
					checkpoint,
					wait_hint: Duration::from_secs(10),
					process_id: None,
				});
				checkpoint += 1;
			}
		});

		let daemon_result = crate::daemon::run_with_shutdown(config, shutdown_rx).await;

		// Cancel the status update task
		status_task.abort();
		daemon_result
	});

	// Tell Windows we're stopping
	let final_state = if result.is_ok() {
		info!("service stopping normally");
		ServiceStatus {
			service_type: SERVICE_TYPE,
			current_state: ServiceState::Stopped,
			controls_accepted: ServiceControlAccept::empty(),
			exit_code: ServiceExitCode::Win32(0),
			checkpoint: 0,
			wait_hint: Duration::default(),
			process_id: None,
		}
	} else {
		error!("service stopping with error: {result:?}");
		ServiceStatus {
			service_type: SERVICE_TYPE,
			current_state: ServiceState::Stopped,
			controls_accepted: ServiceControlAccept::empty(),
			exit_code: ServiceExitCode::Win32(1),
			checkpoint: 0,
			wait_hint: Duration::default(),
			process_id: None,
		}
	};

	status_handle
		.set_service_status(final_state)
		.into_diagnostic()?;

	result
}

///
/// Checks for common issues like Service Control Manager availability,
/// disk space, and executable accessibility.
fn run_diagnostics() {
	println!("Running diagnostics...\n");

	// Check if SCM is running
	print!("Checking Windows Service Control Manager... ");
	match Command::new("sc").args(&["query"]).output() {
		Ok(output) if output.status.success() => {
			println!("✓ Running");
		}
		_ => {
			println!("✗ May not be accessible");
			println!("  Tip: Restart the 'Service Control Manager' service from Services.msc\n");
		}
	}

	// Whether the service already exists (install reconciles it either way).
	print!("Checking if service already exists... ");
	match Command::new("sc").args(&["query", SERVICE_NAME]).output() {
		Ok(output) if output.status.success() => {
			println!("• Already installed — its configuration will be updated");
		}
		_ => {
			println!("✓ Not yet installed — it will be created");
		}
	}

	// Check executable path
	print!("Checking executable accessibility... ");
	match std::env::current_exe() {
		Ok(path) => {
			if path.exists() {
				println!("✓ Executable found at: {}", path.display());
			} else {
				println!("✗ Executable path invalid");
				println!("  Path: {}\n", path.display());
			}
		}
		Err(e) => {
			println!("✗ Cannot determine executable path: {}\n", e);
		}
	}

	println!();
}

/// The directory the service writes its (daily-rotating, JSON) logs to.
///
/// Under `%ProgramData%\bestool` like the rest of bestool's state (backups,
/// registration), rather than a separate `BES\bestool-alertd` tree.
fn get_service_log_path() -> Result<std::path::PathBuf> {
	use std::path::PathBuf;

	let log_dir = std::env::var("ProgramData")
		.map(PathBuf::from)
		.unwrap_or_else(|_| PathBuf::from("C:\\ProgramData"))
		.join("bestool")
		.join("logs");

	// lloggs writes into this directory, so make sure it exists.
	if !log_dir.exists() {
		std::fs::create_dir_all(&log_dir).ok();
	}

	Ok(log_dir)
}

/// Install the alertd daemon as a Windows service.
///
/// Creates a Windows service named 'bestool-alertd' that will start automatically.
/// After installation, starts the service immediately.
///
/// # Errors
///
/// Returns an error if the service cannot be created, configured, or started.
pub fn install_service() -> Result<()> {
	install_service_with_args(&[OsString::from("service")])
}

/// Install the alertd daemon as a Windows service with custom launch arguments.
///
/// Creates a Windows service named 'bestool-alertd' that will start automatically.
/// After installation, starts the service immediately.
///
/// # Arguments
///
/// * `launch_arguments` - Command-line arguments to pass when starting the service
///
/// # Errors
///
/// Returns an error if the service cannot be created, configured, or started.
pub fn install_service_with_args(launch_arguments: &[OsString]) -> Result<()> {
	run_diagnostics();

	// CONNECT + CREATE_SERVICE covers both opening an existing service to reconcile
	// it and creating a new one.
	let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
	let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
		.map_err(|e| {
			let error_msg = e.to_string();
			if error_msg.contains("Access is denied") || error_msg.contains("ERROR_ACCESS_DENIED") {
				miette!("Failed to connect to service manager: {}\n\nThis requires administrator privileges. Please run this command in an Administrator command prompt or PowerShell.", error_msg)
			} else {
				miette!("Failed to connect to service manager: {}\n\nTroubleshoot:\n  - Ensure you have administrator privileges\n  - Check that Windows Service Control Manager is running\n  - Try running this command in an Administrator command prompt", error_msg)
			}
		})?;

	let service_binary_path = std::env::current_exe()
		.map_err(|e| miette!("Failed to get current executable path: {}\n\nTroubleshoot:\n  - Ensure the bestool executable is accessible\n  - Check that the path is readable and not corrupted", e))?;

	let log_path = get_service_log_path()?;
	let service_info = desired_service_info(
		service_binary_path,
		desired_launch_arguments(&log_path, launch_arguments),
	);

	// Upsert: reconcile an existing service in place, or create it. Either way the
	// end state is a service with the correct binary, arguments, and startup type.
	let service_access = ServiceAccess::QUERY_CONFIG
		| ServiceAccess::CHANGE_CONFIG
		| ServiceAccess::START
		| ServiceAccess::QUERY_STATUS;
	let service = match service_manager.open_service(SERVICE_NAME, service_access) {
		Ok(existing) => {
			reconcile_service(&existing, &service_info)?;
			existing
		}
		Err(err) if is_service_absent(&err) => create_service(&service_manager, &service_info)?,
		Err(err) => {
			let error_msg = err.to_string();
			if error_msg.contains("Access is denied") || error_msg.contains("ERROR_ACCESS_DENIED") {
				bail!(
					"Failed to open the existing service: {error_msg}\n\nThis requires administrator privileges. Please run this command in an Administrator command prompt or PowerShell."
				);
			}
			bail!("Failed to open the existing service: {error_msg}");
		}
	};

	// Description and failure actions aren't part of ServiceInfo/change_config, so
	// (re)apply them explicitly; both are idempotent.
	service
		.set_description(SERVICE_DESCRIPTION)
		.map_err(|e| miette!("Failed to set service description: {e}"))?;
	apply_failure_actions(&service)?;

	ensure_running(&service)?;

	println!("\nService installed and started successfully!");
	println!("\nTo monitor the service:");
	println!("  • Open Services.msc and find 'BES Alert Daemon'");
	println!("  • Check status and startup type (should be 'Automatic')");
	println!("\nService logs:");
	println!("  • Location: {}", log_path.display());
	println!("  • Logs are stored in JSON format with timestamps");
	println!("\nFor errors:");
	println!("  • Check the log files in the directory above");
	println!(
		"  • Or check Windows Event Viewer: Windows Logs > System (search for 'bestool-alertd')"
	);
	Ok(())
}

/// The launch arguments the service should run with: `--log-file <dir>` (so logs
/// land in the standard location) followed by the daemon's own arguments.
fn desired_launch_arguments(log_path: &std::path::Path, args: &[OsString]) -> Vec<OsString> {
	let mut out = Vec::with_capacity(args.len() + 2);
	out.push(OsString::from("--log-file"));
	out.push(log_path.as_os_str().to_owned());
	out.extend_from_slice(args);
	out
}

/// The service configuration install should converge on.
fn desired_service_info(
	executable_path: std::path::PathBuf,
	launch_arguments: Vec<OsString>,
) -> ServiceInfo {
	ServiceInfo {
		name: OsString::from(SERVICE_NAME),
		display_name: OsString::from(SERVICE_DISPLAY_NAME),
		service_type: SERVICE_TYPE,
		start_type: ServiceStartType::AutoStart,
		error_control: ServiceErrorControl::Normal,
		executable_path,
		launch_arguments,
		dependencies: vec![],
		account_name: None,
		account_password: None,
	}
}

/// Whether an `open_service` error means the service simply isn't installed yet
/// (so install should create it) rather than a real failure.
fn is_service_absent(err: &windows_service::Error) -> bool {
	let msg = err.to_string();
	msg.contains("does not exist")
		|| msg.contains("ERROR_SERVICE_DOES_NOT_EXIST")
		|| msg.contains("not found")
}

/// Create the service fresh.
fn create_service(
	manager: &ServiceManager,
	info: &ServiceInfo,
) -> Result<windows_service::service::Service> {
	let access = ServiceAccess::QUERY_CONFIG
		| ServiceAccess::CHANGE_CONFIG
		| ServiceAccess::START
		| ServiceAccess::QUERY_STATUS;
	manager.create_service(info, access).map_err(|e| {
		let error_msg = e.to_string();
		if error_msg.contains("marked for deletion") || error_msg.contains("ERROR_SERVICE_MARKED_FOR_DELETE") {
			miette!("The service is marked for deletion (a previous removal hasn't completed). Please restart Windows and try again.")
		} else if error_msg.contains("Access is denied") || error_msg.contains("ERROR_ACCESS_DENIED") {
			miette!("Failed to create service: {error_msg}\n\nThis requires administrator privileges. Please run this command in an Administrator command prompt or PowerShell.")
		} else {
			miette!("Failed to create service: {error_msg}")
		}
	})
}

/// Bring an existing service's configuration in line with `desired`. `change_config`
/// is idempotent, so this always applies the correct binary path, arguments, and
/// startup type; the pre-check just reports what's changing.
fn reconcile_service(
	service: &windows_service::service::Service,
	desired: &ServiceInfo,
) -> Result<()> {
	if let Ok(current) = service.query_config() {
		// `executable_path` from the SCM is the whole command line as one string;
		// the desired command line is the exe plus its arguments.
		let have = current.executable_path.to_string_lossy();
		let want = desired.executable_path.to_string_lossy();
		if !have.contains(want.as_ref()) {
			info!("updating service binary path: {have}");
		} else {
			info!("reconciling existing service configuration");
		}
	}
	service
		.change_config(desired)
		.map_err(|e| miette!("Failed to update the existing service configuration: {e}"))?;
	Ok(())
}

/// Start the service if it isn't already running and wait for it to reach the
/// Running state.
fn ensure_running(service: &windows_service::service::Service) -> Result<()> {
	if let Ok(status) = service.query_status()
		&& status.current_state == ServiceState::Running
	{
		return Ok(());
	}

	if let Err(e) = service.start::<&OsStr>(&[]) {
		let msg = e.to_string();
		if msg.contains("already running") || msg.contains("ERROR_SERVICE_ALREADY_RUNNING") {
			// A concurrent start won the race; the poll below confirms Running.
		} else if msg.contains("marked for deletion")
			|| msg.contains("ERROR_SERVICE_MARKED_FOR_DELETE")
		{
			bail!(
				"Failed to start service: the service is marked for deletion. Please restart Windows and try again."
			);
		} else {
			bail!(
				"Failed to start service: {msg}\n\nTroubleshoot:\n  - Check Windows Event Viewer under Windows Logs > System\n  - Verify the bestool executable path is correct and accessible"
			);
		}
	}

	print!("Waiting for service to start");
	let max_wait = Duration::from_secs(30);
	let start = std::time::Instant::now();
	loop {
		std::thread::sleep(Duration::from_millis(500));
		print!(".");
		std::io::Write::flush(&mut std::io::stdout()).ok();
		match service.query_status() {
			Ok(status) if status.current_state == ServiceState::Running => {
				println!(" ✓");
				return Ok(());
			}
			Ok(_) => {}
			Err(e) => {
				println!();
				bail!("Failed to query service status while waiting for startup: {e}");
			}
		}
		if start.elapsed() > max_wait {
			println!();
			bail!(
				"Service failed to reach Running state within 30 seconds. Check Windows Event Viewer for details."
			);
		}
	}
}

/// Uninstall the alertd Windows service.
///
/// Stops the 'bestool-alertd' Windows service if running, then removes it.
///
/// # Errors
///
/// Returns an error if the service cannot be opened, stopped, or deleted.
pub fn uninstall_service() -> Result<()> {
	let manager_access = ServiceManagerAccess::CONNECT;
	let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
		.map_err(|e| {
			let error_msg = e.to_string();
			if error_msg.contains("Access is denied") || error_msg.contains("ERROR_ACCESS_DENIED") {
				miette!("Failed to connect to service manager: {}\n\nThis requires administrator privileges. Please run this command in an Administrator command prompt or PowerShell.", error_msg)
			} else {
				miette!("Failed to connect to service manager: {}\n\nTroubleshoot:\n  - Ensure you have administrator privileges\n  - Check that Windows Service Control Manager is running", error_msg)
			}
		})?;

	let service_access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
	let service = service_manager
		.open_service("bestool-alertd", service_access)
		.map_err(|e| {
			let error_msg = e.to_string();
			if error_msg.contains("not found") || error_msg.contains("ERROR_SERVICE_DOES_NOT_EXIST") {
				miette!("Service 'bestool-alertd' not found.\n\nThe service doesn't appear to be installed. No action needed.")
			} else if error_msg.contains("Access is denied") || error_msg.contains("ERROR_ACCESS_DENIED") {
				miette!("Failed to open service: {}\n\nThis requires administrator privileges. Please run this command in an Administrator command prompt or PowerShell.", error_msg)
			} else {
				miette!("Failed to open service: {}\n\nTroubleshoot:\n  - Ensure you have administrator privileges\n  - Verify the service 'bestool-alertd' is installed", error_msg)
			}
		})?;

	// Check current service state
	let service_status = service.query_status().ok();
	if let Some(status) = service_status {
		if status.current_state != ServiceState::Stopped {
			// Try to stop the service, but warn on error rather than fail
			match service.stop() {
				Ok(_) => {
					// Wait a moment for the service to stop
					std::thread::sleep(Duration::from_millis(500));
				}
				Err(e) => {
					let error_msg = e.to_string();
					if error_msg.contains("not running")
						|| error_msg.contains("ERROR_SERVICE_NOT_ACTIVE")
					{
						// Service is already stopped, that's fine
					} else {
						// Warn but continue with deletion
						eprintln!("Warning: Failed to stop service cleanly: {}", error_msg);
						eprintln!("  Attempting to delete service anyway...");
					}
				}
			}
		}
	}

	// Attempt to delete the service regardless of stop result
	service
		.delete()
		.map_err(|e| {
			let error_msg = e.to_string();
			if error_msg.contains("marked for deletion") || error_msg.contains("ERROR_SERVICE_MARKED_FOR_DELETE") {
				miette!("Service is already marked for deletion. It will be removed after the next restart.")
			} else {
				miette!("Failed to delete service: {}\n\nTroubleshoot:\n  - Ensure no processes are using this service\n  - The service may need to be restarted first\n  - Check Windows Event Viewer for more details\n  - You may need to restart Windows and try again", error_msg)
			}
		})?;

	println!("Service stopped and uninstalled successfully");
	Ok(())
}

/// Configure failure recovery actions on an existing Windows service.
///
/// Opens the already-installed 'bestool-alertd' service and updates its failure
/// recovery settings to automatically restart on failure.
///
/// # Errors
///
/// Returns an error if the service cannot be opened or configured.
pub fn configure_recovery() -> Result<()> {
	let manager_access = ServiceManagerAccess::CONNECT;
	let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
		.map_err(|e| {
			let error_msg = e.to_string();
			if error_msg.contains("Access is denied") || error_msg.contains("ERROR_ACCESS_DENIED") {
				miette!("Failed to connect to service manager: {}\n\nThis requires administrator privileges. Please run this command in an Administrator command prompt or PowerShell.", error_msg)
			} else {
				miette!("Failed to connect to service manager: {}", error_msg)
			}
		})?;

	let service_access = ServiceAccess::QUERY_CONFIG | ServiceAccess::CHANGE_CONFIG;
	let service = service_manager
		.open_service(SERVICE_NAME, service_access)
		.map_err(|e| {
			let error_msg = e.to_string();
			if error_msg.contains("not found") || error_msg.contains("ERROR_SERVICE_DOES_NOT_EXIST")
			{
				miette!(
					"Service 'bestool-alertd' not found.\n\nInstall the service first with: bestool-alertd install"
				)
			} else if error_msg.contains("Access is denied")
				|| error_msg.contains("ERROR_ACCESS_DENIED")
			{
				miette!(
					"Failed to open service: {}\n\nThis requires administrator privileges.",
					error_msg
				)
			} else {
				miette!("Failed to open service: {}", error_msg)
			}
		})?;

	apply_failure_actions(&service)?;

	println!("Failure recovery actions configured successfully");
	println!("  1st failure: restart after 10 seconds");
	println!("  2nd failure: restart after 30 seconds");
	println!("  3rd+ failure: restart after 60 seconds");
	println!("  Reset counter after: 24 hours");
	Ok(())
}

/// Check whether the service has failure recovery actions configured.
///
/// Returns `Ok(true)` if at least one restart action is configured and
/// failure-actions-on-non-crash-failures is enabled. Returns `Ok(false)`
/// if the service exists but recovery is not (fully) configured.
///
/// # Errors
///
/// Returns an error if the service cannot be opened or queried.
pub fn is_recovery_configured() -> Result<bool> {
	let manager_access = ServiceManagerAccess::CONNECT;
	let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
		.map_err(|e| miette!("Failed to connect to service manager: {}", e))?;

	let service_access = ServiceAccess::QUERY_CONFIG;
	let service = service_manager
		.open_service(SERVICE_NAME, service_access)
		.map_err(|e| {
			let error_msg = e.to_string();
			if error_msg.contains("not found") || error_msg.contains("ERROR_SERVICE_DOES_NOT_EXIST")
			{
				miette!("Service 'bestool-alertd' not found.")
			} else {
				miette!("Failed to open service: {}", error_msg)
			}
		})?;

	let actions = service
		.get_failure_actions()
		.map_err(|e| miette!("Failed to query failure actions: {}", e))?;

	let has_restart_action = actions.actions.as_ref().is_some_and(|a| {
		a.iter()
			.any(|act| act.action_type == ServiceActionType::Restart)
	});

	let non_crash_enabled = service
		.get_failure_actions_on_non_crash_failures()
		.unwrap_or(false);

	Ok(has_restart_action && non_crash_enabled)
}

fn apply_failure_actions(service: &windows_service::service::Service) -> Result<()> {
	let failure_actions = ServiceFailureActions {
		reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
		reboot_msg: None,
		command: None,
		actions: Some(vec![
			ServiceAction {
				action_type: ServiceActionType::Restart,
				delay: Duration::from_secs(10),
			},
			ServiceAction {
				action_type: ServiceActionType::Restart,
				delay: Duration::from_secs(30),
			},
			ServiceAction {
				action_type: ServiceActionType::Restart,
				delay: Duration::from_secs(60),
			},
		]),
	};
	service
		.update_failure_actions(failure_actions)
		.map_err(|e| miette!("Failed to configure failure recovery actions: {}", e))?;

	service
		.set_failure_actions_on_non_crash_failures(true)
		.map_err(|e| {
			miette!(
				"Failed to enable failure actions on non-crash failures: {}",
				e
			)
		})?;

	Ok(())
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	use super::*;

	#[test]
	fn launch_arguments_prepend_the_log_file_flag() {
		let args = desired_launch_arguments(
			Path::new(r"C:\ProgramData\bestool\logs"),
			&[OsString::from("alertd"), OsString::from("service")],
		);
		assert_eq!(
			args,
			vec![
				OsString::from("--log-file"),
				OsString::from(r"C:\ProgramData\bestool\logs"),
				OsString::from("alertd"),
				OsString::from("service"),
			]
		);
	}
}
