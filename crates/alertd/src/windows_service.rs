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

use miette::{IntoDiagnostic, Result, miette};
use tracing::{error, info};
use windows_service::{
	define_windows_service,
	service::{
		ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
		ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
	},
	service_control_handler::{self, ServiceControlHandlerResult},
	service_dispatcher,
	service_manager::{ServiceManager, ServiceManagerAccess},
};

use crate::DaemonConfig;

const SERVICE_NAME: &str = "bestool-alertd";
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
			loop {
				tokio::time::sleep(Duration::from_secs(5)).await;
				// Send checkpoint updates to keep Windows from timing out
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

/// Performs pre-flight checks before attempting to install the service.
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

	// Check if service already exists
	print!("Checking if service already exists... ");
	match Command::new("sc").args(&["query", SERVICE_NAME]).output() {
		Ok(output) if output.status.success() => {
			println!("✗ Service already exists");
			println!("  Tip: Run 'bestool tamanu alertd uninstall' first\n");
		}
		_ => {
			println!("✓ Service not found (good)");
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

/// Get the log file path for the Windows service.
///
/// Returns a path in the Windows ProgramData directory.
fn get_service_log_path() -> Result<std::path::PathBuf> {
	use std::path::PathBuf;

	// Use ProgramData directory for service logs (typically C:\ProgramData)
	let log_dir = std::env::var("ProgramData")
		.map(PathBuf::from)
		.unwrap_or_else(|_| PathBuf::from("C:\\ProgramData"));

	let log_dir = log_dir.join("BES").join("bestool-alertd");

	// Try to create the directory if it doesn't exist
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

	// Get log file path and append logging arguments
	let log_path = get_service_log_path()?;
	let mut final_arguments = launch_arguments.to_vec();
	final_arguments.push(OsString::from("--log-file"));
	final_arguments.push(OsString::from(log_path.as_os_str()));

	let service_info = ServiceInfo {
		name: OsString::from("bestool-alertd"),
		display_name: OsString::from("BES Alert Daemon"),
		service_type: ServiceType::OWN_PROCESS,
		start_type: ServiceStartType::AutoStart,
		error_control: ServiceErrorControl::Normal,
		executable_path: service_binary_path,
		launch_arguments: final_arguments,
		dependencies: vec![],
		account_name: None,
		account_password: None,
	};

	let service = service_manager
		.create_service(
			&service_info,
			ServiceAccess::CHANGE_CONFIG | ServiceAccess::START,
		)
		.map_err(|e| {
			let error_msg = e.to_string();
			if error_msg.contains("Already exists") || error_msg.contains("ERROR_SERVICE_EXISTS") {
				miette!("Service 'bestool-alertd' already exists.\n\nTroubleshoot:\n  - To reinstall, run: bestool tamanu alertd uninstall\n  - Then run: bestool tamanu alertd install")
			} else if error_msg.contains("Access is denied") || error_msg.contains("ERROR_ACCESS_DENIED") {
				miette!("Failed to create service: {}\n\nThis requires administrator privileges. Please run this command in an Administrator command prompt or PowerShell.", error_msg)
			} else {
				miette!("Failed to create service: {}\n\nTroubleshoot:\n  - Restart the 'Service Control Manager' service (Services.msc)\n  - Restart Windows if the problem persists\n  - Check Windows Event Viewer > Windows Logs > System for related errors\n  - Verify the service name 'bestool-alertd' is not reserved or in-use\n  - Try running: 'sc query bestool-alertd' to check service state\n  - Try running: 'sc delete bestool-alertd' if service is marked for deletion", error_msg)
			}
		})?;

	service
		.set_description("Monitors and executes alert definitions from configuration files")
		.map_err(|e| miette!("Failed to set service description: {}\n\nThe service was created but configuration failed. Please try uninstalling and reinstalling.", e))?;

	service
		.start::<&OsStr>(&[])
		.map_err(|e| {
			let error_msg = e.to_string();
			if error_msg.contains("marked for deletion") || error_msg.contains("ERROR_SERVICE_MARKED_FOR_DELETE") {
				miette!("Failed to start service: {}\n\nThe service is marked for deletion. Please restart Windows and try again.", error_msg)
			} else {
				miette!("Failed to start service: {}\n\nTroubleshoot:\n  - Check Windows Event Viewer under Windows Logs > System\n  - Verify the bestool executable path is correct and accessible\n  - Ensure no other service is using the same name\n  - Try starting the service manually using Services.msc", error_msg)
			}
		})?;

	// Wait for the service to reach Running state
	print!("Waiting for service to start");
	let max_wait = Duration::from_secs(30);
	let start = std::time::Instant::now();
	let poll_interval = Duration::from_millis(500);

	loop {
		std::thread::sleep(poll_interval);
		print!(".");
		std::io::Write::flush(&mut std::io::stdout()).ok();

		match service.query_status() {
			Ok(status) => {
				if status.current_state == ServiceState::Running {
					println!(" ✓");
					break;
				}
			}
			Err(e) => {
				println!();
				return Err(miette!("Failed to query service status while waiting for startup: {}", e));
			}
		}

		if start.elapsed() > max_wait {
			println!();
			return Err(miette!("Service failed to reach Running state within 30 seconds. Check Windows Event Viewer for details."));
		}
	}

	let log_path = get_service_log_path()?;
	println!("\nService installed and started successfully!");
	println!("\nTo monitor the service:");
	println!("  • Open Services.msc and find 'BES Alert Daemon'");
	println!("  • Check status and startup type (should be 'Automatic')");
	println!("\nService logs:");
	println!("  • Location: {}", log_path.display());
	println!("  • Logs are stored in JSON format with timestamps");
	println!("\nFor errors:");
	println!("  • Check the log files in the directory above");
	println!("  • Or check Windows Event Viewer: Windows Logs > System (search for 'bestool-alertd')");
	Ok(())
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
					if error_msg.contains("not running") || error_msg.contains("ERROR_SERVICE_NOT_ACTIVE") {
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
