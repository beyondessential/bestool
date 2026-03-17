use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use jiff::Timestamp;

use crate::{http_server::state::ServerState, metrics};

pub async fn handle_health(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
	let last_activity = metrics::last_activity_timestamp();
	let now = Timestamp::now().as_second();
	let started_at = state.started_at.as_second();

	// If no activity has been recorded yet, check against startup time
	// (alerts may not have ticked yet if the daemon just started)
	let reference_time = if last_activity == 0 {
		started_at
	} else {
		last_activity
	};

	let elapsed_secs = now.saturating_sub(reference_time);

	let healthy = match state.watchdog_timeout {
		Some(timeout) => elapsed_secs < timeout.as_secs() as i64,
		None => true,
	};

	let body = serde_json::json!({
		"healthy": healthy,
		"last_activity_secs_ago": if last_activity == 0 { None } else { Some(now.saturating_sub(last_activity)) },
		"uptime_secs": now.saturating_sub(started_at),
		"watchdog_timeout_secs": state.watchdog_timeout.map(|t| t.as_secs()),
	});

	let status = if healthy {
		StatusCode::OK
	} else {
		StatusCode::from_u16(530).expect("530 is a valid status code")
	};

	(status, Json(body))
}

#[cfg(test)]
mod tests {
	use axum::{extract::State, http::StatusCode, response::IntoResponse};

	use super::*;
	use crate::http_server::test_utils::create_test_state;

	#[tokio::test]
	async fn test_health_endpoint_reports_healthy() {
		let state = create_test_state().await;
		let response = handle_health(State(state)).await.into_response();
		assert_eq!(response.status(), StatusCode::OK);

		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
		assert_eq!(json["healthy"], true);
		assert!(json["uptime_secs"].is_number());
		assert!(json["watchdog_timeout_secs"].is_number());
	}
}
