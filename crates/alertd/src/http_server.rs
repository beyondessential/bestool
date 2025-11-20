//! HTTP server for alertd daemon control and metrics.
//!
//! Provides a simple HTTP API listening on localhost:8271 with the following endpoints:
//! - `GET /`: List of available endpoints
//! - `POST /reload`: Trigger a configuration reload (equivalent to SIGHUP)
//! - `GET /metrics`: Prometheus-formatted metrics for monitoring
//! - `GET /status`: Daemon status information in JSON format

use std::sync::Arc;

use axum::{
	Json, Router,
	extract::State,
	http::StatusCode,
	response::IntoResponse,
	routing::{get, post},
};
use jiff::Timestamp;
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::metrics;

#[derive(Clone)]
pub struct ServerState {
	pub reload_tx: mpsc::Sender<()>,
	pub started_at: Timestamp,
	pub pid: u32,
}

#[derive(Serialize, serde::Deserialize)]
struct StatusResponse {
	name: String,
	version: String,
	started_at: String,
	pid: u32,
}

pub async fn start_server(reload_tx: mpsc::Sender<()>) {
	let started_at = Timestamp::now();
	let pid = std::process::id();

	let state = ServerState {
		reload_tx,
		started_at,
		pid,
	};

	let app = Router::new()
		.route("/", get(handle_index))
		.route("/reload", post(handle_reload))
		.route("/metrics", get(handle_metrics))
		.route("/status", get(handle_status))
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

pub async fn handle_status(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
	let status = StatusResponse {
		name: "bestool-alertd".to_string(),
		version: env!("CARGO_PKG_VERSION").to_string(),
		started_at: state.started_at.to_string(),
		pid: state.pid,
	};
	Json(status)
}

async fn handle_index() -> impl IntoResponse {
	let endpoints = serde_json::json!([
		{
			"method": "GET",
			"path": "/",
			"description": "List of available endpoints"
		},
		{
			"method": "POST",
			"path": "/reload",
			"description": "Trigger a configuration reload (equivalent to SIGHUP)"
		},
		{
			"method": "GET",
			"path": "/metrics",
			"description": "Prometheus-formatted metrics for monitoring"
		},
		{
			"method": "GET",
			"path": "/status",
			"description": "Daemon status information in JSON format"
		}
	]);

	(
		StatusCode::OK,
		[(axum::http::header::CONTENT_TYPE, "application/json")],
		serde_json::to_string_pretty(&endpoints).unwrap(),
	)
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
		let state = Arc::new(ServerState {
			reload_tx,
			started_at: Timestamp::now(),
			pid: std::process::id(),
		});

		let response = handle_reload(State(state)).await.into_response();
		assert_eq!(response.status(), StatusCode::OK);

		// Verify the reload signal was sent
		assert!(reload_rx.try_recv().is_ok());
	}

	#[tokio::test]
	async fn test_status_endpoint() {
		let (reload_tx, _reload_rx) = mpsc::channel::<()>(10);
		let started_at = Timestamp::now();
		let pid = std::process::id();

		let state = Arc::new(ServerState {
			reload_tx,
			started_at,
			pid,
		});

		let response = handle_status(State(state)).await.into_response();
		assert_eq!(response.status(), StatusCode::OK);

		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let status: StatusResponse = serde_json::from_slice(&body).unwrap();

		assert_eq!(status.name, "bestool-alertd");
		assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
		assert_eq!(status.pid, pid);
		assert!(!status.started_at.is_empty());
	}

	#[tokio::test]
	async fn test_index_endpoint() {
		let response = handle_index().await.into_response();
		assert_eq!(response.status(), StatusCode::OK);

		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let endpoints: serde_json::Value = serde_json::from_slice(&body).unwrap();

		assert!(endpoints.is_array());
		let endpoints = endpoints.as_array().unwrap();
		assert_eq!(endpoints.len(), 4);

		// Check that all expected endpoints are present
		let paths: Vec<&str> = endpoints
			.iter()
			.filter_map(|e| e.get("path").and_then(|p| p.as_str()))
			.collect();
		assert!(paths.contains(&"/"));
		assert!(paths.contains(&"/reload"));
		assert!(paths.contains(&"/metrics"));
		assert!(paths.contains(&"/status"));
	}
}
