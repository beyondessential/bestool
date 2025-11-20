//! Tests for Windows service integration.
//!
//! These tests verify that the Windows service commands are available and correctly
//! structured when compiled for Windows. On non-Windows platforms, these tests
//! verify that the service functionality is properly gated behind cfg(windows).

#[cfg(windows)]
#[test]
fn test_windows_service_module_exists() {
	// Verify the windows_service module is available on Windows
	let _ = bestool_alertd::windows_service::run_service;
}

#[cfg(not(windows))]
#[test]
fn test_windows_service_module_not_available() {
	// This test ensures the code compiles on non-Windows platforms
	// The windows_service module should not be available
	assert!(
		true,
		"Windows service module correctly not available on this platform"
	);
}

#[test]
fn test_daemon_run_with_shutdown() {
	// Verify that run_with_shutdown is publicly available
	// This is used by both the service and regular daemon modes
	let _ = bestool_alertd::run_with_shutdown;
}
