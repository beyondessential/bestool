mod loaded_alerts;
mod pause;
mod reload;
mod validate;

pub use loaded_alerts::get_loaded_alerts;
pub use pause::pause_alert;
pub use reload::send_reload;
pub use validate::validate_alert;

use tracing::info;

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
	let client = reqwest::Client::new();
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
