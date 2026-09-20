use std::{collections::BTreeMap, sync::Arc};

use axum::{
	Json,
	body::Body,
	extract::{ConnectInfo, Path, Query, State},
	http::{HeaderValue, Method, StatusCode, header::CONTENT_TYPE},
	response::{IntoResponse, Response},
};
use futures::StreamExt;
use tracing::warn;

use crate::alertd::{
	certificates::{delivery::Endpoints, peer},
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
///
/// An endpoint marked `guarded` changes state beyond the daemon, so it answers
/// only to a POST from the superuser. The daemon binds loopback, which makes
/// every process on the host a caller: without this, any unprivileged user could
/// repoint a production DNS record, and a GET could be fired by a page loaded in
/// a browser on the box.
pub async fn handle_task_endpoint(
	State(state): State<Arc<ServerState>>,
	ConnectInfo(ends): ConnectInfo<Endpoints>,
	method: Method,
	Path((task, endpoint)): Path<(String, String)>,
	Query(query): Query<BTreeMap<String, String>>,
) -> Response {
	let Some(mounted) = state.task_endpoints.get(&(task.clone(), endpoint.clone())) else {
		return (
			StatusCode::NOT_FOUND,
			format!("no endpoint at /tasks/{task}/{endpoint}"),
		)
			.into_response();
	};

	if mounted.guarded {
		if method != Method::POST {
			return (
				StatusCode::METHOD_NOT_ALLOWED,
				format!("/tasks/{task}/{endpoint} changes state, so it takes a POST"),
			)
				.into_response();
		}
		// The superuser only. `--permit-cert-user` names the user the front end
		// runs as so it can fetch a certificate during a handshake; that is not
		// a licence to publish DNS records or spend orders.
		if let Err(err) =
			peer::caller_permitted(peer::Permitted::default(), ends.remote, ends.local).await
		{
			warn!(peer = %ends.remote, path = %format!("/tasks/{task}/{endpoint}"), %err, "refused a request to a guarded task endpoint");
			return (StatusCode::FORBIDDEN, format!("{err}")).into_response();
		}
	}

	let mut ctx = TaskContext::from_internal(&state.internal_context);
	ctx.query = query;
	let response = (mounted.handler)(ctx).await;

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

#[cfg(test)]
mod tests {
	use std::collections::HashMap;

	use axum::{Router, routing::get};
	use serde_json::json;

	use super::*;
	use crate::alertd::tasks::TaskEndpoint;

	/// This process's own uid, read the same way the guard reads a caller's:
	/// without libc, which the workspace forbids reaching for.
	#[cfg(target_os = "linux")]
	fn own_uid() -> u32 {
		std::fs::read_to_string("/proc/self/status")
			.unwrap()
			.lines()
			.find_map(|line| line.strip_prefix("Uid:"))
			.and_then(|rest| rest.split_whitespace().next()?.parse().ok())
			.unwrap()
	}

	/// Mount one open and one guarded endpoint on an ephemeral loopback port.
	async fn serve() -> String {
		let answer = |_ctx: TaskContext| {
			Box::pin(async move { TaskEndpointResponse::Json(json!({"ran": true})) })
				as futures::future::BoxFuture<'static, TaskEndpointResponse>
		};

		let mut endpoints = HashMap::new();
		for endpoint in [
			TaskEndpoint::open("status", Arc::new(answer)),
			TaskEndpoint::guarded("mutate", Arc::new(answer)),
		] {
			endpoints.insert(("t".to_string(), endpoint.name.to_string()), endpoint);
		}

		let mut server = crate::alertd::http_server::test_utils::create_test_state().await;
		Arc::get_mut(&mut server).unwrap().task_endpoints = Arc::new(endpoints);

		let app = Router::new()
			.route(
				"/tasks/{task}/{endpoint}",
				get(handle_task_endpoint).post(handle_task_endpoint),
			)
			.with_state(server);
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		tokio::spawn(async move {
			let _ = axum::serve(
				listener,
				app.into_make_service_with_connect_info::<Endpoints>(),
			)
			.await;
		});
		base
	}

	/// An endpoint that only reports is readable by any local caller, over a
	/// GET, as every command that reads one already does.
	#[tokio::test]
	async fn an_open_endpoint_answers_a_plain_get() {
		let base = serve().await;
		let response = reqwest::get(format!("{base}/tasks/t/status"))
			.await
			.unwrap();
		assert_eq!(response.status(), 200);
	}

	/// A GET cannot reach a guarded endpoint, which is what stops a page loaded
	/// in a browser on the host from triggering one with an `<img src>`: a
	/// cross-origin GET needs no CORS permission to take effect.
	#[tokio::test]
	async fn a_guarded_endpoint_refuses_a_get() {
		let base = serve().await;
		let response = reqwest::get(format!("{base}/tasks/t/mutate"))
			.await
			.unwrap();
		assert_eq!(response.status(), 405);
	}

	/// And a POST from anyone but the superuser is refused, so an unprivileged
	/// local process cannot repoint a name or spend an order.
	///
	/// Only Linux can name a caller today. Everywhere else the lookup is not
	/// implemented and the guard refuses whoever asks, which is the safe
	/// direction — a refusal is a misconfiguration to correct, not a silent
	/// grant — so that is what this pins there.
	#[tokio::test]
	async fn a_guarded_endpoint_identifies_its_caller() {
		let base = serve().await;
		let response = reqwest::Client::new()
			.post(format!("{base}/tasks/t/mutate"))
			.send()
			.await
			.unwrap();

		#[cfg(target_os = "linux")]
		let expected = if own_uid() == 0 { 200 } else { 403 };
		#[cfg(not(target_os = "linux"))]
		let expected = 403;

		assert_eq!(response.status(), expected);
	}
}
