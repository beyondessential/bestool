use super::try_connect_daemon;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct AlertStateInfo {
	path: String,
	enabled: bool,
	interval: String,
	triggered_at: Option<String>,
	last_sent_at: Option<String>,
	paused_until: Option<String>,
	always_send: String,
}

/// Get the list of currently loaded alerts from a running daemon
pub async fn get_loaded_alerts(addrs: &[std::net::SocketAddr], detail: bool) -> miette::Result<()> {
	let (client, base_url) = try_connect_daemon(addrs).await?;

	let url = if detail {
		format!("{}/alerts?detail=true", base_url)
	} else {
		format!("{}/alerts", base_url)
	};

	let response = client
		.get(url)
		.send()
		.await
		.map_err(|e| miette::miette!("failed to get alerts: {}", e))?;

	if !response.status().is_success() {
		return Err(miette::miette!(
			"failed to get alerts (status: {})",
			response.status()
		));
	}

	if detail {
		let alert_states: Vec<AlertStateInfo> = response
			.json()
			.await
			.map_err(|e| miette::miette!("failed to parse response: {}", e))?;

		if alert_states.is_empty() {
			println!("No alerts currently loaded");
		} else {
			println!("Loaded alerts ({}):\n", alert_states.len());
			for state in alert_states {
				println!("  {}:", state.path);
				println!("    enabled: {}", state.enabled);
				println!("    interval: {}", state.interval);
				println!("    always_send: {}", state.always_send);
				if let Some(triggered_at) = state.triggered_at {
					println!("    triggered_at: {}", triggered_at);
				}
				if let Some(last_sent_at) = state.last_sent_at {
					println!("    last_sent_at: {}", last_sent_at);
				}
				if let Some(paused_until) = state.paused_until {
					println!("    paused_until: {}", paused_until);
				}
				println!();
			}
		}
	} else {
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
	}

	Ok(())
}
