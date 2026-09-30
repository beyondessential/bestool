//! Clients for the daemon's control HTTP API.

use tracing::info;

pub use control::{reload, restart};
pub use logs::{LogsArgs, show_logs};
pub use status::get_status;

mod control;
mod logs;
mod status;

/// A failed update, as reported by the daemon's self-update status endpoint.
///
/// spec: UPD#update-failures
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedUpdateFailure {
	pub version: String,
	/// Absent from a daemon that records only the version.
	pub reason: Option<String>,
}

impl ReportedUpdateFailure {
	fn from_status(body: &serde_json::Value) -> Option<Self> {
		let field = |key| body.get(key).and_then(serde_json::Value::as_str);
		Some(Self {
			version: field("failed_version")?.to_owned(),
			reason: field("failed_reason").map(str::to_owned),
		})
	}
}

/// Fetch the update, if any, that the daemon's self-update task last failed to
/// install. `None` when there is no failure, and also when the daemon has no
/// self-update task (it runs only on Windows) or can't be reached.
pub async fn fetch_update_failure(
	client: &reqwest::Client,
	base_url: &str,
) -> Option<ReportedUpdateFailure> {
	let response = client
		.get(format!("{base_url}/tasks/self-update/status"))
		.send()
		.await
		.ok()?;
	if !response.status().is_success() {
		return None;
	}
	let body: serde_json::Value = response.json().await.ok()?;
	ReportedUpdateFailure::from_status(&body)
}

/// Default server addresses to try when connecting to the daemon
pub fn default_server_addrs() -> Vec<std::net::SocketAddr> {
	vec![
		"[::1]:8271".parse().unwrap(),
		"127.0.0.1:8271".parse().unwrap(),
	]
}

/// Attempt to connect to a running daemon at any of the provided addresses
///
/// Returns a tuple of (client, base_url) on success, or an error if no daemon could be reached.
pub async fn try_connect_daemon(
	addrs: &[std::net::SocketAddr],
) -> miette::Result<(reqwest::Client, String)> {
	let client = crate::alertd::http_client();
	let mut last_error = None;

	for addr in addrs {
		let url = format!("http://{}", addr);
		info!("trying to connect to daemon at {}", url);

		// Try to connect with a simple status check
		let test_response = match client.get(format!("{}/status", url)).send().await {
			Ok(resp) => resp,
			Err(e) => {
				info!("failed to connect to {}: {}", url, e);
				last_error = Some(e);
				continue;
			}
		};

		if test_response.status().is_success() {
			info!("connected to daemon at {}", url);
			return Ok((client, url));
		}
	}

	if let Some(err) = last_error {
		Err(miette::miette!(
			"failed to connect to daemon at any of {} address(es): {}",
			addrs.len(),
			err
		))
	} else {
		Err(miette::miette!(
			"no daemon found at any of {} address(es)",
			addrs.len()
		))
	}
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	#[test]
	fn update_failure_with_reason() {
		assert_eq!(
			ReportedUpdateFailure::from_status(&json!({
				"current": "2.2.4",
				"failed_version": "2.2.5",
				"failed_reason": "signature did not verify",
			})),
			Some(ReportedUpdateFailure {
				version: "2.2.5".into(),
				reason: Some("signature did not verify".into()),
			})
		);
	}

	#[test]
	fn update_failure_from_a_daemon_without_reasons() {
		assert_eq!(
			ReportedUpdateFailure::from_status(&json!({
				"current": "2.2.4",
				"failed_version": "2.2.5",
			})),
			Some(ReportedUpdateFailure {
				version: "2.2.5".into(),
				reason: None,
			})
		);
	}

	#[test]
	fn no_update_failure() {
		assert_eq!(
			ReportedUpdateFailure::from_status(&json!({
				"current": "2.2.4",
				"failed_version": null,
				"failed_reason": null,
			})),
			None
		);
	}
}
