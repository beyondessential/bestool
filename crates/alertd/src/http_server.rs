//! HTTP server for alertd daemon control and metrics.
//!
//! Provides a simple HTTP API listening on [::1]:8271 and 127.0.0.1:8271 by default
//! with the following endpoints:
//! - `GET /`: List of available endpoints
//! - `POST /reload`: Trigger a configuration reload (equivalent to SIGHUP)
//! - `POST /alert`: Trigger a custom HTTP alert
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
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::warn;
use tracing::{error, info};

use crate::{
	EmailConfig,
	alert::InternalContext,
	events::{EventContext, EventManager, EventType},
	metrics,
	scheduler::Scheduler,
};

#[derive(Clone)]
pub struct ServerState {
	pub reload_tx: mpsc::Sender<()>,
	pub started_at: Timestamp,
	pub pid: u32,
	pub event_manager: Option<Arc<EventManager>>,
	pub internal_context: Arc<InternalContext>,
	pub email_config: Option<EmailConfig>,
	pub dry_run: bool,
	pub scheduler: Arc<Scheduler>,
}

#[derive(Serialize, Deserialize)]
struct StatusResponse {
	name: String,
	version: String,
	started_at: String,
	pid: u32,
}

#[derive(Deserialize)]
struct AlertRequest {
	message: String,
	#[serde(default)]
	subject: Option<String>,
	#[serde(flatten)]
	custom: serde_json::Value,
}

#[derive(Deserialize)]
struct PauseAlertRequest {
	alert: String,
	until: String,
}

#[derive(Serialize)]
struct ValidationResponse {
	valid: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	error: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	error_location: Option<ErrorLocation>,
	#[serde(skip_serializing_if = "Option::is_none")]
	info: Option<ValidationInfo>,
}

#[derive(Serialize)]
struct ErrorLocation {
	line: usize,
	column: usize,
	path: String,
}

#[derive(Serialize)]
struct ValidationInfo {
	enabled: bool,
	interval: String,
	source_type: String,
	targets: usize,
}

pub async fn start_server(
	reload_tx: mpsc::Sender<()>,
	event_manager: Option<Arc<EventManager>>,
	internal_context: Arc<InternalContext>,
	email_config: Option<EmailConfig>,
	dry_run: bool,
	addrs: Vec<std::net::SocketAddr>,
	scheduler: Arc<Scheduler>,
) {
	let started_at = Timestamp::now();
	let pid = std::process::id();

	let state = ServerState {
		reload_tx,
		started_at,
		pid,
		event_manager,
		internal_context,
		email_config,
		dry_run,
		scheduler,
	};

	let app = Router::new()
		.route("/", get(handle_index))
		.route("/reload", post(handle_reload))
		.route("/alert", post(handle_alert))
		.route("/alerts", get(handle_alerts).delete(handle_pause_alert))
		.route("/validate", post(handle_validate))
		.route("/metrics", get(handle_metrics))
		.route("/status", get(handle_status))
		.with_state(Arc::new(state));

	// Use default if no addresses provided
	let addrs_to_try = if addrs.is_empty() {
		vec![
			"[::1]:8271".parse().unwrap(),
			"127.0.0.1:8271".parse().unwrap(),
		]
	} else {
		addrs
	};

	let mut listener = None;
	let mut last_error = None;

	// Try each address in order until one succeeds
	for addr in &addrs_to_try {
		match tokio::net::TcpListener::bind(addr).await {
			Ok(l) => {
				info!("HTTP server listening on http://{}", addr);
				listener = Some(l);
				break;
			}
			Err(e) => {
				warn!("failed to bind HTTP server to {}: {}", addr, e);
				last_error = Some(e);
			}
		}
	}

	let listener = match listener {
		Some(l) => l,
		None => {
			if let Some(e) = last_error {
				warn!("failed to bind HTTP server to any address: {}", e);
			} else {
				warn!("no addresses provided for HTTP server");
			}
			warn!("waiting 10 seconds before continuing without");
			warn!("use --no-server to disable the HTTP server and this warning");

			tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

			info!("continuing without HTTP server");
			return;
		}
	};

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

async fn handle_alert(
	State(state): State<Arc<ServerState>>,
	Json(payload): Json<AlertRequest>,
) -> impl IntoResponse {
	info!(message = %payload.message, "received HTTP alert");

	let event_context = EventContext::Http {
		message: payload.message,
		subject: payload.subject,
		custom: payload.custom,
	};

	if let Some(ref event_mgr) = state.event_manager {
		match event_mgr
			.trigger_event(
				EventType::Http,
				&state.internal_context,
				state.email_config.as_ref(),
				state.dry_run,
				event_context,
			)
			.await
		{
			Ok(()) => {
				info!("HTTP alert triggered successfully");
				(StatusCode::OK, "Alert triggered\n")
			}
			Err(e) => {
				error!("failed to trigger HTTP alert: {e:?}");
				(
					StatusCode::INTERNAL_SERVER_ERROR,
					"Failed to trigger alert\n",
				)
			}
		}
	} else {
		error!("no event manager available");
		(
			StatusCode::SERVICE_UNAVAILABLE,
			"Event manager not available\n",
		)
	}
}

async fn handle_alerts(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
	let files = state.scheduler.get_loaded_alerts().await;
	let alerts: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
	Json(alerts)
}

async fn handle_pause_alert(
	State(state): State<Arc<ServerState>>,
	Json(payload): Json<PauseAlertRequest>,
) -> impl IntoResponse {
	use std::path::PathBuf;

	info!(alert = %payload.alert, until = %payload.until, "pausing alert");

	let until = match payload.until.parse::<jiff::Timestamp>() {
		Ok(ts) => ts,
		Err(e) => {
			error!("failed to parse timestamp: {e:?}");
			return (
				StatusCode::BAD_REQUEST,
				format!("Invalid timestamp: {}\n", e),
			)
				.into_response();
		}
	};

	let path = PathBuf::from(&payload.alert);
	match state.scheduler.pause_alert(&path, until).await {
		Ok(true) => {
			info!("alert paused successfully");
			(StatusCode::OK, "Alert paused\n").into_response()
		}
		Ok(false) => {
			info!("alert not found");
			(StatusCode::NOT_FOUND, "Alert not found\n").into_response()
		}
		Err(e) => {
			error!("failed to pause alert: {e:?}");
			(StatusCode::INTERNAL_SERVER_ERROR, "Failed to pause alert\n").into_response()
		}
	}
}

async fn handle_validate(body: String) -> impl IntoResponse {
	use crate::alert::{AlertDefinition, TicketSource};

	// Try to parse as YAML with serde_path_to_error for better error messages
	let deserializer = serde_yaml::Deserializer::from_str(&body);
	let alert: AlertDefinition = match serde_path_to_error::deserialize(deserializer) {
		Ok(alert) => alert,
		Err(err) => {
			// Parse error - return detailed error information
			let path = err.path().to_string();
			let inner = err.into_inner();
			let error_msg = format!("{}", inner);

			// The inner error is already a serde_yaml::Error, extract location if available
			// Note: serde_yaml::Error doesn't expose location() in all cases
			let response = ValidationResponse {
				valid: false,
				error: Some(format!("Parse error at '{}': {}", path, error_msg)),
				error_location: None, // Location info is included in the error message
				info: None,
			};

			return (StatusCode::OK, Json(response)).into_response();
		}
	};

	// Try to normalize the alert (this validates send targets and other fields)
	let external_targets = std::collections::HashMap::new();
	match alert.normalise(&external_targets) {
		Ok((alert, resolved_targets)) => {
			let source_type = match &alert.source {
				TicketSource::Sql { .. } => "sql",
				TicketSource::Shell { .. } => "shell",
				TicketSource::Event { .. } => "event",
				TicketSource::None => "none",
			}
			.to_string();

			let response = ValidationResponse {
				valid: true,
				error: None,
				error_location: None,
				info: Some(ValidationInfo {
					enabled: alert.enabled,
					interval: alert.interval.clone(),
					source_type,
					targets: resolved_targets.len(),
				}),
			};

			(StatusCode::OK, Json(response)).into_response()
		}
		Err(err) => {
			// Normalization error (e.g., invalid interval, missing targets)
			let response = ValidationResponse {
				valid: false,
				error: Some(format!("Validation error: {:#}", err)),
				error_location: None,
				info: None,
			};

			(StatusCode::OK, Json(response)).into_response()
		}
	}
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

#[cfg(test)]
mod tests {
	use super::*;

	async fn create_test_state() -> Arc<ServerState> {
		let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
		let pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-test")
			.await
			.unwrap();
		let ctx = Arc::new(InternalContext {
			pg_pool: pool.clone(),
		});

		let scheduler = Arc::new(crate::scheduler::Scheduler::new(
			vec![],
			ctx.clone(),
			None,
			true,
		));

		let (reload_tx, _reload_rx) = mpsc::channel::<()>(10);
		Arc::new(ServerState {
			reload_tx,
			started_at: Timestamp::now(),
			pid: std::process::id(),
			event_manager: None,
			internal_context: ctx,
			email_config: None,
			dry_run: true,
			scheduler,
		})
	}

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
		let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
		let pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-test")
			.await
			.unwrap();
		let ctx = Arc::new(InternalContext {
			pg_pool: pool.clone(),
		});

		let scheduler = Arc::new(crate::scheduler::Scheduler::new(
			vec![],
			ctx.clone(),
			None,
			true,
		));

		let (reload_tx, mut reload_rx) = mpsc::channel::<()>(10);
		let state = Arc::new(ServerState {
			reload_tx,
			started_at: Timestamp::now(),
			pid: std::process::id(),
			event_manager: None,
			internal_context: ctx,
			email_config: None,
			dry_run: true,
			scheduler,
		});

		let response = handle_reload(State(state)).await.into_response();
		assert_eq!(response.status(), StatusCode::OK);

		// Verify the reload signal was sent
		assert!(reload_rx.try_recv().is_ok());
	}

	#[tokio::test]
	async fn test_status_endpoint() {
		let state = create_test_state().await;

		let response = handle_status(State(state.clone())).await.into_response();
		assert_eq!(response.status(), StatusCode::OK);

		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let status: StatusResponse = serde_json::from_slice(&body).unwrap();

		assert_eq!(status.name, "bestool-alertd");
		assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
		assert_eq!(status.pid, state.pid);
		assert!(!status.started_at.is_empty());
	}

	#[tokio::test]
	async fn test_alert_endpoint_no_event_manager() {
		let state = create_test_state().await;

		let payload = AlertRequest {
			message: "Test message".to_string(),
			subject: Some("Test subject".to_string()),
			custom: serde_json::json!({"extra": "data"}),
		};

		let response = handle_alert(State(state), Json(payload))
			.await
			.into_response();

		assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
	}

	#[tokio::test]
	async fn test_alert_endpoint_with_event_manager() {
		use std::collections::HashMap;

		let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
		let pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-test")
			.await
			.unwrap();
		let ctx = Arc::new(InternalContext {
			pg_pool: pool.clone(),
		});

		// Create a mock event manager
		let event_manager = crate::events::EventManager::new(Vec::new(), &HashMap::new());

		let scheduler = Arc::new(crate::scheduler::Scheduler::new(
			vec![],
			ctx.clone(),
			None,
			true,
		));

		let (reload_tx, _reload_rx) = mpsc::channel::<()>(10);
		let state = Arc::new(ServerState {
			reload_tx,
			started_at: Timestamp::now(),
			pid: std::process::id(),
			event_manager: Some(Arc::new(event_manager)),
			internal_context: ctx,
			email_config: None,
			dry_run: true,
			scheduler,
		});

		let payload = AlertRequest {
			message: "Test alert message".to_string(),
			subject: Some("Test alert".to_string()),
			custom: serde_json::json!({"priority": "high", "source": "test"}),
		};

		let response = handle_alert(State(state), Json(payload))
			.await
			.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let body_str = String::from_utf8(body.to_vec()).unwrap();
		assert_eq!(body_str, "Alert triggered\n");
	}

	#[tokio::test]
	async fn test_alerts_endpoint() {
		let state = create_test_state().await;

		let response = handle_alerts(State(state)).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let alerts: Vec<String> = serde_json::from_slice(&body).unwrap();

		// Should be empty for test state
		assert!(alerts.is_empty());
	}
}
