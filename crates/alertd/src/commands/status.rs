use miette::miette;
use tracing::info;

use super::try_connect_daemon;
use crate::http_server::StatusResponse;

/// Print the daemon's status. `local_version`, when given, is the version of the
/// `bestool` binary invoking this (not this crate's version); the daemon reports
/// the same kind of version, so a difference means the on-disk binary is newer
/// than the running daemon. `None` skips that comparison (e.g. the reprint right
/// after a restart, when a difference is expected and about to resolve).
pub async fn get_status(
	addrs: &[std::net::SocketAddr],
	local_version: Option<&str>,
) -> miette::Result<()> {
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

	// Destructure exhaustively (no `..`): a new StatusResponse field then fails to
	// compile here until it's surfaced below.
	let StatusResponse {
		name,
		version,
		started_at,
		pid,
		backups_running,
		backups_configured,
	} = status;

	println!("Name:      {name}");
	println!("Version:   {version}");
	if let Some(local_version) = local_version
		&& version != local_version
	{
		println!(
			"           WARNING: running daemon is {version}, but this CLI is {local_version}"
		);
		println!("           Run `bestool alertd restart` to pick up the new version.");
	}
	println!("PID:       {pid}");
	println!("Started:   {started_at}");

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

	if backups_configured.is_empty() {
		println!("Backups:   none configured");
	} else {
		println!("Backups:   {}", backups_configured.join(", "));
	}
	if !backups_running.is_empty() {
		println!("  running: {}", backups_running.len());
		for backup in &backups_running {
			let descr = describe_latest(&backup.latest);
			let run_id = backup.run_id.as_deref().unwrap_or("-");
			println!(
				"           - {} [{descr}] run {run_id}, started {}",
				backup.r#type, backup.started_at
			);
		}
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

/// A short human descriptor of a running backup's latest status event, so the
/// status line shows *which* phase it's in (e.g. `phase: snapshot`) or the live
/// kopia progress — not just the bare event type.
fn describe_latest(latest: &serde_json::Value) -> String {
	let field = |key| latest.get(key).and_then(serde_json::Value::as_str);
	match field("event") {
		Some("phase") => format!("phase: {}", field("phase").unwrap_or("?")),
		Some("progress") => match field("status").unwrap_or("") {
			"" => "progress".to_string(),
			status if status.chars().count() > 60 => {
				format!("{}…", status.chars().take(59).collect::<String>())
			}
			status => status.to_string(),
		},
		Some(other) => other.to_string(),
		None => "?".to_string(),
	}
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	#[test]
	fn describe_latest_names_the_phase() {
		assert_eq!(
			describe_latest(&json!({"event": "phase", "phase": "snapshot"})),
			"phase: snapshot"
		);
	}

	#[test]
	fn describe_latest_shows_progress_and_truncates() {
		assert_eq!(
			describe_latest(&json!({"event": "progress", "status": "hashed 3 files"})),
			"hashed 3 files"
		);
		let long = "x".repeat(100);
		let out = describe_latest(&json!({"event": "progress", "status": long}));
		assert!(out.ends_with('…'));
		assert_eq!(out.chars().count(), 60);
	}

	#[test]
	fn describe_latest_falls_back_to_event_type() {
		assert_eq!(describe_latest(&json!({"event": "started"})), "started");
		assert_eq!(describe_latest(&json!({})), "?");
	}
}
