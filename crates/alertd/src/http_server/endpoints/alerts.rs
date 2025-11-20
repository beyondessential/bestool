use std::sync::Arc;

use axum::{
	Json,
	extract::{Query, State},
	response::IntoResponse,
};

use crate::http_server::{
	state::ServerState,
	types::{AlertStateInfo, AlertsQuery},
};

pub async fn handle_alerts(
	State(state): State<Arc<ServerState>>,
	Query(query): Query<AlertsQuery>,
) -> impl IntoResponse {
	if query.detail {
		let states = state.scheduler.get_alert_states().await;
		let mut alert_states: Vec<AlertStateInfo> = states
			.iter()
			.map(|(path, state)| {
				let always_send = match &state.definition.always_send {
					crate::alert::AlwaysSend::Boolean(true) => "true".to_string(),
					crate::alert::AlwaysSend::Boolean(false) => "false".to_string(),
					crate::alert::AlwaysSend::Timed(config) => {
						format!("after: {}", config.after)
					}
				};

				AlertStateInfo {
					path: path.display().to_string(),
					enabled: state.definition.enabled,
					interval: state.definition.interval.clone(),
					triggered_at: state.triggered_at.map(|t| t.to_string()),
					last_sent_at: state.last_sent_at.map(|t| t.to_string()),
					paused_until: state.paused_until.map(|t| t.to_string()),
					always_send,
				}
			})
			.collect();
		alert_states.sort_by(|a, b| a.path.cmp(&b.path));
		Json(alert_states).into_response()
	} else {
		let files = state.scheduler.get_loaded_alerts().await;
		let alerts: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
		Json(alerts).into_response()
	}
}

#[cfg(test)]
mod tests {
	use axum::{
		extract::{Query, State},
		http::StatusCode,
		response::IntoResponse,
	};

	use super::*;
	use crate::http_server::test_utils::create_test_state;

	#[tokio::test]
	async fn test_alerts_endpoint() {
		let state = create_test_state().await;

		let query = Query(AlertsQuery { detail: false });
		let response = handle_alerts(State(state), query).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let alerts: Vec<String> = serde_json::from_slice(&body).unwrap();

		// Should be empty for test state
		assert!(alerts.is_empty());
	}

	#[tokio::test]
	async fn test_alerts_endpoint_with_detail() {
		let state = create_test_state().await;

		let query = Query(AlertsQuery { detail: true });
		let response = handle_alerts(State(state), query).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let alert_states: Vec<AlertStateInfo> = serde_json::from_slice(&body).unwrap();

		// Should be empty for test state
		assert!(alert_states.is_empty());
	}
}
