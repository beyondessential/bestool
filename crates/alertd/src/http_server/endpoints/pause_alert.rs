use std::{path::PathBuf, sync::Arc};

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use tracing::{error, info};

use crate::http_server::{state::ServerState, types::PauseAlertRequest};

pub async fn handle_pause_alert(
	State(state): State<Arc<ServerState>>,
	Json(payload): Json<PauseAlertRequest>,
) -> impl IntoResponse {
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
