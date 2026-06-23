//! Control endpoints: `/reload` and `/restart`.

use std::sync::Arc;

use axum::{Json, extract::State, response::IntoResponse};
use serde_json::json;
use tracing::info;

use crate::http_server::state::ServerState;

/// `POST /reload` — refresh runtime state (re-register backup capabilities, pick
/// up `/etc/bestool/backups` changes) without restarting.
pub async fn handle_reload(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
	info!("reload requested via HTTP");
	state.control.reload();
	Json(json!({ "reloading": true }))
}

/// `POST /restart` — ask the daemon to exit so the service manager restarts it
/// (e.g. to pick up a new binary). Triggered after a short delay so this
/// response reaches the client before the daemon exits.
pub async fn handle_restart(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
	info!("restart requested via HTTP");
	let control = state.control.clone();
	tokio::spawn(async move {
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
		control.request_restart().await;
	});
	Json(json!({ "restarting": true }))
}
