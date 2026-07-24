use std::sync::Arc;

use axum::{
	extract::{RawQuery, State},
	http::{HeaderMap, StatusCode, header},
	response::IntoResponse,
};
use tracing::error;

use crate::http_server::{ServerState, metrics_render};
use crate::metrics;

/// Media type a munin plugin sends in `Accept` to select munin-native output.
/// There is no registered type for munin's format; this endpoint and the
/// bundled plugin agree on it privately.
const MUNIN_MEDIA_TYPE: &str = "text/x-munin";

/// Prometheus text exposition content type.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

pub async fn handle_metrics(
	State(state): State<Arc<ServerState>>,
	headers: HeaderMap,
	RawQuery(query): RawQuery,
) -> impl IntoResponse {
	let wants_munin = headers
		.get(header::ACCEPT)
		.and_then(|v| v.to_str().ok())
		.is_some_and(|accept| accept.contains(MUNIN_MEDIA_TYPE));

	let snapshot = match &state.metrics {
		Some(handle) => handle.snapshot().await,
		None => None,
	};

	if wants_munin {
		// Munin's two-call protocol: `?config` asks for field metadata, a bare
		// request for values.
		let config = query
			.as_deref()
			.is_some_and(|q| q.split('&').any(|p| p == "config"));
		let body = metrics_render::render_munin(
			snapshot.as_ref(),
			metrics::last_activity_timestamp(),
			config,
		);
		return ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response();
	}

	// Prometheus: keep the existing registry gauge output verbatim (so existing
	// scrapers are undisturbed), then append the sweep-derived metrics.
	let mut body = match metrics::gather_metrics() {
		Ok(text) => text,
		Err(e) => {
			error!("failed to gather metrics: {e:?}");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				format!("Failed to gather metrics: {e}\n"),
			)
				.into_response();
		}
	};
	if let Some(snapshot) = &snapshot {
		body.push_str(&metrics_render::render_prometheus(snapshot));
	}
	([(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)], body).into_response()
}

#[cfg(test)]
mod tests {
	use std::{collections::HashMap, sync::Arc, time::Duration};

	use axum::{
		body::to_bytes,
		extract::{RawQuery, State},
		http::{HeaderMap, HeaderValue, StatusCode, header},
		response::IntoResponse,
	};
	use jiff::Timestamp;

	use super::*;
	use crate::{context::InternalContext, http_server::ServerState};

	/// The prometheus registry is process-global and initialises exactly once;
	/// guard it so parallel tests don't double-init.
	fn init_metrics_once() {
		static INIT: std::sync::Once = std::sync::Once::new();
		INIT.call_once(crate::metrics::init_metrics);
	}

	/// A minimal server state with no doctor metrics handle and no database —
	/// enough to exercise the endpoint's format negotiation.
	fn state() -> Arc<ServerState> {
		let ctx = Arc::new(InternalContext {
			pg_pool: None,
			http_client: reqwest::Client::new(),
			canopy_client: None,
			reload: tokio::sync::watch::channel(0).1,
			restart: None,
		});
		Arc::new(ServerState {
			started_at: Timestamp::now(),
			pid: std::process::id(),
			binary_version: "0.0.0-test".to_string(),
			internal_context: ctx,
			watchdog_timeout: Some(Duration::from_secs(600)),
			task_endpoints: Arc::new(HashMap::new()),
			control: crate::daemon::DaemonControl::detached(),
			backups: None,
			metrics: None,
		})
	}

	async fn body_of(response: axum::response::Response) -> String {
		let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
		String::from_utf8(body.to_vec()).unwrap()
	}

	#[tokio::test]
	async fn default_is_prometheus() {
		init_metrics_once();
		let response = handle_metrics(State(state()), HeaderMap::new(), RawQuery(None))
			.await
			.into_response();
		assert_eq!(response.status(), StatusCode::OK);
		assert_eq!(
			response.headers().get(header::CONTENT_TYPE).unwrap(),
			PROMETHEUS_CONTENT_TYPE
		);
		assert!(body_of(response).await.contains("# HELP"));
	}

	#[tokio::test]
	async fn accept_munin_selects_munin() {
		init_metrics_once();
		let mut headers = HeaderMap::new();
		headers.insert(header::ACCEPT, HeaderValue::from_static(MUNIN_MEDIA_TYPE));
		let response = handle_metrics(State(state()), headers, RawQuery(None))
			.await
			.into_response();
		let body = body_of(response).await;
		// No sweep handle wired: liveness graph only, no census.
		assert!(body.contains("multigraph bes_alertd_daemon"));
		assert!(!body.contains("multigraph bes_alertd_checks"));
	}

	#[tokio::test]
	async fn munin_config_query_is_honoured() {
		init_metrics_once();
		let mut headers = HeaderMap::new();
		headers.insert(header::ACCEPT, HeaderValue::from_static(MUNIN_MEDIA_TYPE));
		let response = handle_metrics(State(state()), headers, RawQuery(Some("config".into())))
			.await
			.into_response();
		let body = body_of(response).await;
		assert!(body.contains("graph_category bestool"));
		assert!(!body.contains(".value "));
	}
}
