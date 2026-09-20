use std::{collections::HashMap, sync::Arc};

use jiff::Timestamp;

use crate::alertd::context::InternalContext;

use super::ServerState;

/// A server state for the endpoint tests. The endpoints it serves report on the
/// daemon itself — uptime, watchdog, version — and never reach the database, so
/// this needs no connection.
pub async fn create_test_state() -> Arc<ServerState> {
	let ctx = Arc::new(InternalContext {
		http_client: reqwest::Client::new(),
		canopy_client: None,
		reload: tokio::sync::watch::channel(0).1,
		#[cfg(windows)]
		restart: None,
	});

	Arc::new(ServerState {
		started_at: Timestamp::now(),
		pid: std::process::id(),
		binary_version: "0.0.0-test".to_string(),
		internal_context: ctx,
		watchdog_timeout: Some(std::time::Duration::from_secs(600)),
		task_endpoints: Arc::new(HashMap::new()),
		control: crate::alertd::daemon::DaemonControl::detached(),
		backups: None,
		metrics: None,
		certificates: None,
	})
}
