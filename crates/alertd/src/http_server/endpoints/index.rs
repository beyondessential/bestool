use axum::{http::StatusCode, response::IntoResponse};

pub async fn handle_index() -> impl IntoResponse {
	let endpoints = serde_json::json!([
		{
			"method": "GET",
			"path": "/",
			"description": "List of available endpoints"
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
		},
		{
			"method": "GET",
			"path": "/health",
			"description": "Health check endpoint (returns 200 if healthy, 530 if stalled)"
		},
		{
			"method": "GET",
			"path": "/tasks/{task}/{endpoint}",
			"description": "Invoke an endpoint exposed by a registered background task"
		}
	]);

	(
		StatusCode::OK,
		[(axum::http::header::CONTENT_TYPE, "application/json")],
		serde_json::to_string_pretty(&endpoints).unwrap(),
	)
}
