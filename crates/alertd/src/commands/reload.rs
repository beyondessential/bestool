use tracing::info;

/// Send a reload signal to a running alertd daemon
///
/// Tries to connect to the daemon's HTTP API at each of the provided addresses in order
/// until one succeeds. This is an alternative to SIGHUP that works on all platforms
/// including Windows.
pub async fn send_reload(addrs: &[std::net::SocketAddr]) -> miette::Result<()> {
	let client = reqwest::Client::new();

	let mut last_error = None;

	for addr in addrs {
		let url = format!("http://{}", addr);
		info!("checking if daemon is running at {}", url);

		// First, check if daemon is running by fetching status
		let status_response = match client.get(format!("{}/status", url)).send().await {
			Ok(resp) => resp,
			Err(e) => {
				info!("failed to connect to {}: {}", url, e);
				last_error = Some(e);
				continue;
			}
		};

		if !status_response.status().is_success() {
			info!(
				"daemon at {} returned status: {}",
				url,
				status_response.status()
			);
			continue;
		}

		let status: serde_json::Value = match status_response.json().await {
			Ok(s) => s,
			Err(e) => {
				info!("failed to parse status response from {}: {}", url, e);
				continue;
			}
		};

		// Verify it's the correct daemon
		if status.get("name").and_then(|n| n.as_str()) != Some("bestool-alertd") {
			info!(
				"unexpected daemon running at {}: {:?}",
				url,
				status.get("name")
			);
			continue;
		}

		info!(
			"found bestool-alertd daemon at {} (pid: {})",
			url,
			status.get("pid").unwrap_or(&serde_json::Value::Null)
		);

		// Send reload request
		info!("sending reload request to {}", url);
		let reload_response = match client.post(format!("{}/reload", url)).send().await {
			Ok(resp) => resp,
			Err(e) => {
				return Err(miette::miette!("reload request to {} failed: {}", url, e));
			}
		};

		if !reload_response.status().is_success() {
			return Err(miette::miette!(
				"reload request to {} failed (status: {})",
				url,
				reload_response.status()
			));
		}

		info!("reload request sent successfully to {}", url);
		return Ok(());
	}

	// If we get here, we couldn't connect to any address
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
