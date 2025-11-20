use axum::{http::StatusCode, response::IntoResponse};
use tracing::error;

use crate::metrics;

pub async fn handle_metrics() -> impl IntoResponse {
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

#[cfg(test)]
mod tests {
	use axum::{http::StatusCode, response::IntoResponse};

	use super::*;

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

		// Basic check that it returns Prometheus-formatted metrics
		assert!(body_str.contains("# HELP"));
	}
}
