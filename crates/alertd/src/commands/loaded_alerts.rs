use super::try_connect_daemon;

/// Get the list of currently loaded alerts from a running daemon
pub async fn get_loaded_alerts(addrs: &[std::net::SocketAddr]) -> miette::Result<()> {
	let (client, base_url) = try_connect_daemon(addrs).await?;

	let response = client
		.get(format!("{}/alerts", base_url))
		.send()
		.await
		.map_err(|e| miette::miette!("failed to get alerts: {}", e))?;

	if !response.status().is_success() {
		return Err(miette::miette!(
			"failed to get alerts (status: {})",
			response.status()
		));
	}

	let alerts: Vec<String> = response
		.json()
		.await
		.map_err(|e| miette::miette!("failed to parse response: {}", e))?;

	if alerts.is_empty() {
		println!("No alerts currently loaded");
	} else {
		println!("Loaded alerts ({}):", alerts.len());
		for alert in alerts {
			println!("  {}", alert);
		}
	}

	Ok(())
}
