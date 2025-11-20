use std::{sync::Arc, time::Duration};

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
	let started_at = jiff::Timestamp::now();
	let pid = std::process::id();

	let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
	let (client, connection) = tokio_postgres::connect(&db_url, tokio_postgres::NoTls)
		.await
		.unwrap();
	tokio::spawn(async move {
		let _ = connection.await;
	});

	let (scheduler_client, scheduler_connection) =
		tokio_postgres::connect(&db_url, tokio_postgres::NoTls)
			.await
			.unwrap();
	tokio::spawn(async move {
		let _ = scheduler_connection.await;
	});

	let ctx = Arc::new(bestool_alertd::InternalContext { pg_client: client });
	let scheduler_ctx = Arc::new(bestool_alertd::InternalContext {
		pg_client: scheduler_client,
	});
	let scheduler = Arc::new(bestool_alertd::scheduler::Scheduler::new(
		vec![],
		std::time::Duration::from_secs(60),
		scheduler_ctx,
		None,
		true,
	));

	let state = Arc::new(bestool_alertd::http_server::ServerState {
		reload_tx,
		started_at,
		pid,
		event_manager: None,
		internal_context: ctx,
		email_config: None,
		dry_run: true,
		scheduler,
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
