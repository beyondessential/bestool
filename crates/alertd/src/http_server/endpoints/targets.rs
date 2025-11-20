use std::sync::Arc;

use axum::{Json, extract::State, response::IntoResponse};

use crate::http_server::state::ServerState;

pub async fn handle_targets(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
	let targets = state.scheduler.get_external_targets().await;
	Json(targets)
}

#[cfg(test)]
mod tests {
	use axum::{extract::State, http::StatusCode, response::IntoResponse};

	use super::*;
	use crate::http_server::test_utils::create_test_state;

	#[tokio::test]
	async fn test_targets_endpoint() {
		let state = create_test_state().await;

		let response = handle_targets(State(state)).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
	}
}
