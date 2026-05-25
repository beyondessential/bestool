use std::sync::Arc;

use axum::{
	Json,
	body::Body,
	extract::{Path, State},
	http::{HeaderValue, StatusCode, header::CONTENT_TYPE},
	response::{IntoResponse, Response},
};
use futures::StreamExt;
use tracing::warn;

use crate::{
	http_server::state::ServerState,
	tasks::{TaskContext, TaskEndpointResponse},
};

/// Route handler for `/tasks/:task/:endpoint`.
///
/// Looks up the handler the named background task exposed (via
/// `BackgroundTask::http_endpoints`) and invokes it with a fresh
/// `TaskContext` built from the daemon's internal resources. The handler's
/// `TaskEndpointResponse` is serialised to JSON or NDJSON depending on its
/// variant.
pub async fn handle_task_endpoint(
	State(state): State<Arc<ServerState>>,
	Path((task, endpoint)): Path<(String, String)>,
) -> Response {
	let Some(handler) = state.task_endpoints.get(&(task.clone(), endpoint.clone())) else {
		return (
			StatusCode::NOT_FOUND,
			format!("no endpoint at /tasks/{task}/{endpoint}"),
		)
			.into_response();
	};

	let ctx = TaskContext::from_internal(&state.internal_context);
	let response = handler(ctx).await;

	match response {
		TaskEndpointResponse::Json(value) => Json(value).into_response(),
		TaskEndpointResponse::JsonLines(stream) => {
			let body = Body::from_stream(stream.map(|value| {
				// One JSON value per line, NDJSON style. Newline-on-end keeps
				// the last record syntactically self-contained for readers
				// using `read_line`-style framing.
				let mut bytes = serde_json::to_vec(&value).unwrap_or_else(|err| {
					warn!(%err, "could not serialise task endpoint stream value");
					b"{}".to_vec()
				});
				bytes.push(b'\n');
				Ok::<_, std::convert::Infallible>(bytes)
			}));
			let mut response = Response::new(body);
			response.headers_mut().insert(
				CONTENT_TYPE,
				HeaderValue::from_static("application/x-ndjson"),
			);
			response
		}
		TaskEndpointResponse::Error { status, message } => (
			StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
			message,
		)
			.into_response(),
	}
}
