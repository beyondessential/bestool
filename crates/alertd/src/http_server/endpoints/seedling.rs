use axum::{Json, response::IntoResponse};
use bestool_tamanu::seedling::{HostIdentity, host_identity};

use crate::http_server::types::SeedlingResponse;

/// Report whether this host carries a Seedling identity for bestool.
///
/// The identity itself is root-only, so an unprivileged tool cannot tell by
/// itself whether elevating would gain it one. This answers that without
/// disclosing anything: presence, never the key.
///
/// spec: SEED#advertising-the-host-identity
pub async fn handle_seedling() -> impl IntoResponse {
	Json(report(host_identity()))
}

fn report(identity: HostIdentity) -> SeedlingResponse {
	SeedlingResponse {
		host_identity: identity.present(),
	}
}

#[cfg(test)]
mod tests {
	use std::path::PathBuf;

	use axum::{http::StatusCode, response::IntoResponse};

	use super::*;

	#[test]
	fn an_identity_this_daemon_can_read_is_present() {
		let key = PathBuf::from("/etc/bestool/seedling.key");
		assert!(report(HostIdentity::Readable(key)).host_identity);
	}

	#[test]
	fn an_identity_out_of_this_process_reach_is_still_present() {
		// The answer is about the host, not about the asking process: a tool
		// that would have to elevate still needs to know one exists.
		let key = PathBuf::from("/etc/bestool/seedling.key");
		assert!(report(HostIdentity::NeedsElevation(key)).host_identity);
	}

	#[test]
	fn no_identity_reports_absent() {
		assert!(!report(HostIdentity::Absent).host_identity);
	}

	#[tokio::test]
	async fn the_endpoint_answers_with_the_documented_shape() {
		let response = handle_seedling().await.into_response();
		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		// The value depends on the host this runs on; the shape does not.
		serde_json::from_slice::<SeedlingResponse>(&body).unwrap();
	}
}
