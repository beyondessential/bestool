//! HTTP server for alertd daemon control and metrics.

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
	Router,
	routing::{get, post},
};
use jiff::Timestamp;
use tokio::sync::mpsc;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, error, info, warn};

use crate::{
	EmailConfig,
	alert::InternalContext,
	events::EventManager,
	scheduler::Scheduler,
	tasks::{BackgroundTask, TaskEndpointHandler},
};

mod endpoints;
mod state;
#[cfg(test)]
mod test_utils;
mod types;

pub use endpoints::*;
pub use state::ServerState;
pub use types::*;

#[expect(
	clippy::too_many_arguments,
	reason = "server startup needs all these pieces"
)]
pub async fn start_server(
	reload_tx: mpsc::Sender<()>,
	event_manager: Option<Arc<EventManager>>,
	internal_context: Arc<InternalContext>,
	email_config: Option<EmailConfig>,
	dry_run: bool,
	addrs: Vec<std::net::SocketAddr>,
	scheduler: Arc<Scheduler>,
	watchdog_timeout: Option<Duration>,
	background_tasks: &[Arc<dyn BackgroundTask>],
) {
	let started_at = Timestamp::now();
	let pid = std::process::id();

	let task_endpoints = collect_task_endpoints(background_tasks);

	let state = ServerState {
		reload_tx,
		started_at,
		pid,
		event_manager,
		internal_context,
		email_config,
		dry_run,
		scheduler,
		watchdog_timeout,
		task_endpoints: Arc::new(task_endpoints),
	};

	let app = Router::new()
		.route("/", get(handle_index))
		.route("/reload", post(handle_reload))
		.route("/alerts", get(handle_alerts).delete(handle_pause_alert))
		.route("/targets", get(handle_targets))
		.route("/validate", post(handle_validate))
		.route("/metrics", get(handle_metrics))
		.route("/status", get(handle_status))
		.route("/health", get(handle_health))
		.route("/tasks/{task}/{endpoint}", get(handle_task_endpoint))
		.layer(
			TraceLayer::new_for_http()
				.make_span_with(
					DefaultMakeSpan::new()
						.level(Level::INFO)
						.include_headers(false),
				)
				.on_request(|request: &axum::http::Request<_>, _span: &tracing::Span| {
					info!(
						method = %request.method(),
						uri = %request.uri(),
						"HTTP request"
					);
				})
				.on_response(
					DefaultOnResponse::new()
						.level(Level::INFO)
						.include_headers(false),
				),
		)
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

fn collect_task_endpoints(
	tasks: &[Arc<dyn BackgroundTask>],
) -> HashMap<(String, String), TaskEndpointHandler> {
	let mut map = HashMap::new();
	for task in tasks {
		let task_name = task.name();
		for endpoint in task.http_endpoints() {
			let key = (task_name.to_string(), endpoint.name.to_string());
			if map.contains_key(&key) {
				warn!(
					task = task_name,
					endpoint = endpoint.name,
					"duplicate task endpoint name; later registration wins"
				);
			}
			info!(
				task = task_name,
				endpoint = endpoint.name,
				path = %format!("/tasks/{task_name}/{}", endpoint.name),
				"mounting task endpoint"
			);
			map.insert(key, endpoint.handler);
		}
	}
	map
}
