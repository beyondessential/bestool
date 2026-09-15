use std::sync::Arc;

use tokio::sync::watch;

use bestool_alertd::canopy::CanopyClient;

/// Shared resources the daemon holds for the lifetime of the process and hands
/// to background tasks and HTTP endpoints.
#[derive(Debug, Clone)]
pub struct InternalContext {
	pub http_client: reqwest::Client,
	pub canopy_client: Option<Arc<CanopyClient>>,
	/// Bumped on each reload request (SIGHUP/SIGUSR1); tasks watch it to refresh
	/// their state without a restart.
	pub reload: watch::Receiver<u64>,
	/// Handle to ask the daemon to restart itself. `None` in detached test
	/// contexts. Used by the self-update task after it replaces the binary,
	/// which only happens on Windows.
	#[cfg(windows)]
	pub restart: Option<crate::alertd::daemon::RestartTrigger>,
}
