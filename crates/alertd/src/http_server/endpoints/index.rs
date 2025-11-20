use axum::{http::StatusCode, response::IntoResponse};

pub async fn handle_index() -> impl IntoResponse {
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
			"method": "POST",
			"path": "/alert",
			"description": "Trigger a custom HTTP alert with JSON payload"
		},
		{
			"method": "GET",
			"path": "/alerts",
			"description": "List currently loaded alert files"
		},
		{
			"method": "DELETE",
			"path": "/alerts",
			"description": "Temporarily pause an alert until the specified timestamp (JSON body: {\"alert\": \"PATH\", \"until\": \"TIMESTAMP\"})"
		},
		{
			"method": "GET",
			"path": "/targets",
			"description": "List all currently loaded external targets"
		},
		{
			"method": "POST",
			"path": "/validate",
			"description": "Validate an alert definition (send YAML as request body, returns validation result as JSON)"
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
