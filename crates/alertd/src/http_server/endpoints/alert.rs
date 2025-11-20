use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use tracing::{error, info};

use crate::{
	events::{EventContext, EventType},
	http_server::{state::ServerState, types::AlertRequest},
};

pub async fn handle_alert(
	State(state): State<Arc<ServerState>>,
	Json(payload): Json<AlertRequest>,
) -> impl IntoResponse {
	info!(message = %payload.message, "received HTTP alert");

	let event_context = EventContext::Http {
		message: payload.message,
		subject: payload.subject,
		custom: payload.custom,
	};

	if let Some(ref event_mgr) = state.event_manager {
		match event_mgr
			.trigger_event(
				EventType::Http,
				&state.internal_context,
				state.email_config.as_ref(),
				state.dry_run,
				event_context,
			)
			.await
		{
			Ok(()) => {
				info!("HTTP alert triggered successfully");
				(StatusCode::OK, "Alert triggered\n")
			}
			Err(e) => {
				error!("failed to trigger HTTP alert: {e:?}");
				(
					StatusCode::INTERNAL_SERVER_ERROR,
					"Failed to trigger alert\n",
				)
			}
		}
	} else {
		error!("no event manager available");
		(
			StatusCode::SERVICE_UNAVAILABLE,
			"Event manager not available\n",
		)
	}
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use axum::{extract::State, http::StatusCode, response::IntoResponse};
	use jiff::Timestamp;
	use tokio::sync::mpsc;

	use super::*;
	use crate::{
		alert::InternalContext, events::EventManager, http_server::test_utils::create_test_state,
		scheduler::Scheduler,
	};

	#[tokio::test]
	async fn test_alert_endpoint_no_event_manager() {
		let state = create_test_state().await;

		let payload = AlertRequest {
			message: "Test alert".to_string(),
			subject: Some("Test subject".to_string()),
			custom: serde_json::json!({"key": "value"}),
		};

		let response = handle_alert(State(state), axum::Json(payload))
			.await
			.into_response();

		assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
	}

	#[tokio::test]
	async fn test_alert_endpoint_with_event_manager() {
		let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
		let pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-test")
			.await
			.unwrap();
		let ctx = Arc::new(InternalContext { pg_pool: pool });
		let scheduler = Arc::new(Scheduler::new(vec![], ctx.clone(), None, true));

		let (reload_tx, _reload_rx) = mpsc::channel::<()>(10);

		let event_manager = EventManager::new(vec![], &std::collections::HashMap::new());

		let state = Arc::new(crate::http_server::state::ServerState {
			reload_tx,
			started_at: Timestamp::now(),
			pid: std::process::id(),
			event_manager: Some(Arc::new(event_manager)),
			internal_context: ctx,
			email_config: None,
			dry_run: true,
			scheduler,
		});

		let payload = AlertRequest {
			message: "Test alert".to_string(),
			subject: Some("Test subject".to_string()),
			custom: serde_json::json!({"key": "value"}),
		};

		let response = handle_alert(State(state), axum::Json(payload))
			.await
			.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let body_str = String::from_utf8(body.to_vec()).unwrap();
		assert_eq!(body_str, "Alert triggered\n");
	}
}
