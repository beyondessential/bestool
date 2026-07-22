use std::{collections::HashMap, sync::Arc};

use jiff::Timestamp;

use crate::context::InternalContext;

use super::ServerState;

pub async fn create_test_state() -> Arc<ServerState> {
	let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
	let pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-test")
		.await
		.unwrap();
	let ctx = Arc::new(InternalContext {
		pg_pool: Some(pool),
		http_client: reqwest::Client::new(),
		canopy_client: None,
		reload: tokio::sync::watch::channel(0).1,
		restart: None,
	});

	Arc::new(ServerState {
		started_at: Timestamp::now(),
		pid: std::process::id(),
		binary_version: "0.0.0-test".to_string(),
		internal_context: ctx,
		watchdog_timeout: Some(std::time::Duration::from_secs(600)),
		task_endpoints: Arc::new(HashMap::new()),
		control: crate::daemon::DaemonControl::detached(),
		backups: None,
	})
}
