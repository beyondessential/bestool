//! HTTP server for alertd daemon control and metrics.
//!
//! Provides a simple HTTP API listening on localhost:8271 with the following endpoints:
//! - `POST /reload`: Trigger a configuration reload (equivalent to SIGHUP)
//! - `GET /metrics`: Prometheus-formatted metrics for monitoring

use std::sync::Arc;

use axum::{
	Router,
	extract::State,
	http::StatusCode,
	response::IntoResponse,
	routing::{get, post},
};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::metrics;

#[derive(Clone)]
pub struct ServerState {
	reload_tx: mpsc::Sender<()>,
}

pub async fn start_server(reload_tx: mpsc::Sender<()>) {
	let state = ServerState { reload_tx };

	let app = Router::new()
		.route("/reload", post(handle_reload))
		.route("/metrics", get(handle_metrics))
		.with_state(Arc::new(state));

	let listener = match tokio::net::TcpListener::bind("127.0.0.1:8271").await {
		Ok(listener) => listener,
		Err(e) => {
			error!("failed to bind HTTP server to 127.0.0.1:8271: {}", e);
			return;
		}
	};

	info!("HTTP server listening on http://127.0.0.1:8271");

	if let Err(e) = axum::serve(listener, app).await {
		error!("HTTP server error: {}", e);
	}
}

async fn handle_reload(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
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

async fn handle_metrics() -> impl IntoResponse {
	match metrics::gather_metrics() {
		Ok(metrics) => (StatusCode::OK, metrics).into_response(),
		Err(e) => {
			error!("failed to gather metrics: {e:?}");
			(
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Failed to gather metrics: {e}\n"),
			)
				.into_response()
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_metrics_endpoint() {
		// Initialize metrics for the test
		crate::metrics::init_metrics();

		let response = handle_metrics().await.into_response();
		assert_eq!(response.status(), StatusCode::OK);

		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let body_str = String::from_utf8(body.to_vec()).unwrap();

		// Check that prometheus metrics are present
		assert!(
			body_str.contains("bes_alertd_alerts_loaded"),
			"body: {body_str}"
		);
		assert!(
			body_str.contains("bes_alertd_alerts_sent_total"),
			"body: {body_str}"
		);
		assert!(
			body_str.contains("bes_alertd_alerts_failed_total"),
			"body: {body_str}"
		);
		assert!(
			body_str.contains("bes_alertd_reloads_total"),
			"body: {body_str}"
		);
	}

	#[tokio::test]
	async fn test_reload_endpoint() {
		let (reload_tx, mut reload_rx) = mpsc::channel::<()>(10);
		let state = Arc::new(ServerState { reload_tx });

		let response = handle_reload(State(state)).await.into_response();
		assert_eq!(response.status(), StatusCode::OK);

		// Verify the reload signal was sent
		assert!(reload_rx.try_recv().is_ok());
	}
}
