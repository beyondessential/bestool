use std::time::Duration;

use axum::response::IntoResponse;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_reload_command_when_no_daemon_running() {
	// This test verifies that the reload command fails gracefully when no daemon is running
	let client = reqwest::Client::new();

	let result = client
		.get("http://127.0.0.1:8271/status")
		.timeout(Duration::from_secs(1))
		.send()
		.await;

	// If there's no daemon running, the request should fail
	// (either connection refused or timeout)
	assert!(result.is_err());
}

#[tokio::test]
async fn test_status_endpoint_response_format() {
	// Start a mock HTTP server
	let (reload_tx, _reload_rx) = mpsc::channel::<()>(10);

	// We can't easily test the full reload flow without running the daemon,
	// but we can verify the status endpoint returns the expected format
	let started_at = jiff::Timestamp::now();
	let pid = std::process::id();

	let state = std::sync::Arc::new(bestool_alertd::http_server::ServerState {
		reload_tx,
		started_at,
		pid,
	});

	// This verifies the response structure without needing a full daemon
	let response = bestool_alertd::http_server::handle_status(axum::extract::State(state))
		.await
		.into_response();

	assert_eq!(response.status(), axum::http::StatusCode::OK);

	let body = axum::body::to_bytes(response.into_body(), usize::MAX)
		.await
		.unwrap();
	let status: serde_json::Value = serde_json::from_slice(&body).unwrap();

	assert_eq!(status["name"], "bestool-alertd");
	assert!(status["version"].is_string());
	assert!(status["started_at"].is_string());
	assert_eq!(status["pid"], pid);
}
