use miette::miette;
use tracing::info;

use super::try_connect_daemon;
use crate::http_server::StatusResponse;

pub async fn get_status(addrs: &[std::net::SocketAddr]) -> miette::Result<()> {
	let (client, url) = try_connect_daemon(addrs).await?;

	// Fetch /status
	let status_response = client
		.get(format!("{url}/status"))
		.send()
		.await
		.map_err(|e| miette!("failed to fetch status: {e}"))?;

	if !status_response.status().is_success() {
		return Err(miette!(
			"status endpoint returned {}",
			status_response.status()
		));
	}

	let status: StatusResponse = status_response
		.json()
		.await
		.map_err(|e| miette!("failed to parse status response: {e}"))?;

	// Fetch /health
	let health_response = client
		.get(format!("{url}/health"))
		.send()
		.await
		.map_err(|e| miette!("failed to fetch health: {e}"))?;

	let health_status_code = health_response.status().as_u16();
	let health: serde_json::Value = health_response
		.json()
		.await
		.map_err(|e| miette!("failed to parse health response: {e}"))?;

	info!("connected to daemon at {url}");

	let local_version = crate::VERSION;

	println!("Name:      {}", status.name);
	println!("Version:   {}", status.version);
	if status.version != local_version {
		println!(
			"           WARNING: running daemon is {}, but this CLI is {local_version}",
			status.version
		);
		println!("           Consider restarting the service to pick up the new version.");
	}
	println!("PID:       {}", status.pid);
	println!("Started:   {}", status.started_at);

	let healthy = health
		.get("healthy")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	if healthy {
		println!("Health:    ok");
	} else {
		println!("Health:    UNHEALTHY (HTTP {health_status_code})");
	}

	if let Some(uptime) = health.get("uptime_secs").and_then(|v| v.as_i64()) {
		println!("Uptime:    {}", format_duration(uptime));
	}

	if let Some(last) = health
		.get("last_activity_secs_ago")
		.and_then(|v| v.as_i64())
	{
		println!("Last tick: {last}s ago");
	} else {
		println!("Last tick: (none yet)");
	}

	if let Some(timeout) = health.get("watchdog_timeout_secs").and_then(|v| v.as_u64()) {
		println!("Watchdog:  {}", format_duration(timeout as i64));
	} else {
		println!("Watchdog:  disabled");
	}

	if !healthy {
		std::process::exit(1);
	}

	Ok(())
}

fn format_duration(secs: i64) -> String {
	let hours = secs / 3600;
	let mins = (secs % 3600) / 60;
	let secs = secs % 60;
	if hours > 0 {
		format!("{hours}h {mins}m {secs}s")
	} else if mins > 0 {
		format!("{mins}m {secs}s")
	} else {
		format!("{secs}s")
	}
}
