use std::io::{self, Write};

use tracing::info;

use super::try_connect_daemon;

/// Pause an alert until a specified time
pub async fn pause_alert(
	alert_path: &str,
	until: Option<&str>,
	addrs: &[std::net::SocketAddr],
) -> miette::Result<()> {
	// Parse or default the until time
	let until_timestamp = if let Some(until_str) = until {
		// Try parsing as timestamp first
		if let Ok(ts) = until_str.parse::<jiff::Timestamp>() {
			ts
		} else {
			// Try parsing as relative time using jiff's Span
			let span: jiff::Span = until_str
				.parse()
				.map_err(|e| miette::miette!("failed to parse time '{}': {}", until_str, e))?;
			jiff::Timestamp::now()
				.checked_add(span)
				.map_err(|e| miette::miette!("time calculation overflow: {}", e))?
		}
	} else {
		// Default to 1 week from now
		jiff::Timestamp::now()
			.checked_add(jiff::Span::new().days(7))
			.map_err(|e| miette::miette!("time calculation overflow: {}", e))?
	};

	let (client, base_url) = try_connect_daemon(addrs).await?;

	// Try to pause the alert
	let url = format!("{}/alerts", base_url);

	let body = serde_json::json!({
		"alert": alert_path,
		"until": until_timestamp.to_string(),
	});

	let response = client
		.delete(&url)
		.json(&body)
		.send()
		.await
		.map_err(|e| miette::miette!("failed to send pause request: {}", e))?;

	if response.status() == reqwest::StatusCode::NOT_FOUND {
		// Alert not found, try to find a partial match
		info!("alert not found, trying to find partial match");

		let alerts_response = client
			.get(format!("{}/alerts", base_url))
			.send()
			.await
			.map_err(|e| miette::miette!("failed to get alerts list: {}", e))?;

		let alerts: Vec<String> = alerts_response
			.json()
			.await
			.map_err(|e| miette::miette!("failed to parse alerts list: {}", e))?;

		// Find partial matches
		let matches: Vec<&String> = alerts.iter().filter(|a| a.contains(alert_path)).collect();

		if matches.is_empty() {
			return Err(miette::miette!(
				"alert '{}' not found and no partial matches",
				alert_path
			));
		} else if matches.len() == 1 {
			// Exactly one match, ask for confirmation
			println!("Alert '{}' not found.", alert_path);
			println!("Did you mean: {}", matches[0]);
			print!("Pause this alert? [y/N] ");
			io::stdout().flush().unwrap();

			let mut input = String::new();
			io::stdin()
				.read_line(&mut input)
				.map_err(|e| miette::miette!("failed to read input: {}", e))?;

			if input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes") {
				// Retry with the matched path
				let retry_url = format!("{}/alerts", base_url);
				let retry_body = serde_json::json!({
					"alert": matches[0],
					"until": until_timestamp.to_string(),
				});

				let retry_response = client
					.delete(&retry_url)
					.json(&retry_body)
					.send()
					.await
					.map_err(|e| miette::miette!("failed to send pause request: {}", e))?;

				if !retry_response.status().is_success() {
					return Err(miette::miette!(
						"failed to pause alert (status: {})",
						retry_response.status()
					));
				}

				println!("Alert paused until {}", until_timestamp);
				return Ok(());
			} else {
				return Err(miette::miette!("pause cancelled"));
			}
		} else {
			// Multiple matches
			println!(
				"Alert '{}' not found. Did you mean one of these?",
				alert_path
			);
			for (i, m) in matches.iter().enumerate() {
				println!("  {}. {}", i + 1, m);
			}
			return Err(miette::miette!(
				"multiple matches found, please be more specific"
			));
		}
	}

	if !response.status().is_success() {
		return Err(miette::miette!(
			"failed to pause alert (status: {})",
			response.status()
		));
	}

	println!("Alert paused until {}", until_timestamp);
	Ok(())
}
