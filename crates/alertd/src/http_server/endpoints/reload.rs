use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse};
use tracing::{error, info};

use crate::http_server::state::ServerState;

pub async fn handle_reload(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
	match state.reload_tx.send(()).await {
		Ok(()) => {
			info!("reload triggered via HTTP");
			(StatusCode::OK, "Reload triggered\n")
		}
		Err(_) => {
			error!("failed to send reload signal");
			(
				StatusCode::INTERNAL_SERVER_ERROR,
				"Failed to trigger reload\n",
			)
		}
	}
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use axum::{extract::State, http::StatusCode, response::IntoResponse};
	use jiff::Timestamp;
	use tokio::sync::mpsc;

	use super::*;
	use crate::{alert::InternalContext, http_server::state::ServerState, scheduler::Scheduler};

	#[tokio::test]
	async fn test_reload_endpoint() {
		let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
		let pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-test")
			.await
			.unwrap();
		let ctx = Arc::new(InternalContext { pg_pool: pool });
		let scheduler = Arc::new(Scheduler::new(vec![], ctx.clone(), None, true));

		let (reload_tx, mut reload_rx) = mpsc::channel::<()>(10);

		let state = Arc::new(ServerState {
			reload_tx,
			started_at: Timestamp::now(),
			pid: std::process::id(),
			event_manager: None,
			internal_context: ctx,
			email_config: None,
			dry_run: true,
			scheduler,
		});

		let response = handle_reload(State(state)).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);

		// Verify the reload signal was sent
		assert!(reload_rx.try_recv().is_ok());
	}
}
