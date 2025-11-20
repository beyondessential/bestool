//! Windows service integration for alertd daemon.
//!
//! This module provides native Windows service support, allowing alertd to be installed,
//! managed, and run as a Windows service through the Service Control Manager (SCM).
//!
//! The service integrates with Windows shutdown signals and properly reports its status
//! to the SCM throughout its lifecycle.

use std::{
	ffi::OsString,
	sync::{Arc, Mutex},
	time::Duration,
};

use miette::{IntoDiagnostic, Result};
use tracing::{error, info};
use windows_service::{
	define_windows_service,
	service::{
		ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
		ServiceType,
	},
	service_control_handler::{self, ServiceControlHandlerResult},
	service_dispatcher,
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
			checkpoint: 0,
			wait_hint: Duration::from_secs(3),
			process_id: None,
		})
		.into_diagnostic()?;

	// Start the daemon in a new tokio runtime
	let runtime = tokio::runtime::Runtime::new().into_diagnostic()?;

	// Tell Windows we're running
	status_handle
		.set_service_status(ServiceStatus {
			service_type: SERVICE_TYPE,
			current_state: ServiceState::Running,
			controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
			exit_code: ServiceExitCode::Win32(0),
			checkpoint: 0,
			wait_hint: Duration::default(),
			process_id: None,
		})
		.into_diagnostic()?;

	info!("service started successfully");

	// Run the daemon
	let result = runtime
		.block_on(async move { crate::daemon::run_with_shutdown(config, shutdown_rx).await });

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
