//! HTTP server for alertd daemon control and metrics.

use std::sync::Arc;

use axum::{
	Router,
	routing::{get, post},
};
use jiff::Timestamp;
use tokio::sync::mpsc;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{Level, error, info, warn};

use crate::{EmailConfig, alert::InternalContext, events::EventManager, scheduler::Scheduler};

mod endpoints;
mod state;
#[cfg(test)]
mod test_utils;
mod types;

pub use endpoints::*;
pub use state::ServerState;
pub use types::*;

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
		.route("/targets", get(handle_targets))
		.route("/validate", post(handle_validate))
		.route("/metrics", get(handle_metrics))
		.route("/status", get(handle_status))
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
