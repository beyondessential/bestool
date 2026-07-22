use std::sync::Arc;

use axum::{Json, extract::State, response::IntoResponse};

use crate::http_server::{state::ServerState, types::StatusResponse};

pub async fn handle_status(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
	let (backups_running, backups_configured) = match &state.backups {
		Some(registry) => (registry.running().await, registry.configured().await),
		None => (Vec::new(), Vec::new()),
	};
	let status = StatusResponse {
		name: "bestool-alertd".to_string(),
		// The bestool binary's version — what self-update targets and what operators
		// recognise — not this crate's independent version.
		version: state.binary_version.clone(),
		started_at: state.started_at.to_string(),
		pid: state.pid,
		backups_running,
		backups_configured,
	};
	Json(status)
}

#[cfg(test)]
mod tests {
	use axum::{extract::State, http::StatusCode, response::IntoResponse};

	use super::*;
	use crate::http_server::test_utils::create_test_state;

	#[tokio::test]
	async fn test_status_endpoint() {
		let state = create_test_state().await;

		let response = handle_status(State(state)).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let status: StatusResponse = serde_json::from_slice(&body).unwrap();

		assert_eq!(status.name, "bestool-alertd");
		assert!(!status.version.is_empty());
	}
}
