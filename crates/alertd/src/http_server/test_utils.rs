use std::sync::Arc;

use jiff::Timestamp;
use tokio::sync::mpsc;

use crate::{alert::InternalContext, scheduler::Scheduler};

use super::ServerState;

pub async fn create_test_state() -> Arc<ServerState> {
	let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
	let pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-test")
		.await
		.unwrap();
	let ctx = Arc::new(InternalContext { pg_pool: pool });
	let scheduler = Arc::new(Scheduler::new(
		vec![],
		ctx.clone(),
		None,
		true, // dry_run
	));

	let (reload_tx, _reload_rx) = mpsc::channel::<()>(10);

	Arc::new(ServerState {
		reload_tx,
		started_at: Timestamp::now(),
		pid: std::process::id(),
		event_manager: None,
		internal_context: ctx,
		email_config: None,
		dry_run: true,
		scheduler,
	})
}
