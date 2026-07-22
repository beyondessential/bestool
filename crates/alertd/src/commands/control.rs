//! Clients for the daemon's `/reload` and `/restart` control endpoints.

use std::time::Duration;

use miette::miette;
use tracing::info;

use super::try_connect_daemon;

/// After a reload/restart, pause then print the daemon status, so the operator
/// sees the resulting state. Retries while the daemon is unreachable (e.g. a
/// restart waits out the unit's `RestartSec` before the daemon is back), up to
/// `attempts` one-second tries; a daemon that never reappears is reported, not
/// treated as a failure of the command (the action itself succeeded).
async fn reprint_status(addrs: &[std::net::SocketAddr], attempts: u32) {
	println!();
	for attempt in 1..=attempts {
		tokio::time::sleep(Duration::from_secs(1)).await;
		match super::get_status(addrs, None).await {
			Ok(()) => return,
			Err(err) if attempt == attempts => {
				println!("(daemon status not available yet: {err})");
			}
			// Not back yet — keep waiting.
			Err(_) => {}
		}
	}
}

/// Ask a running daemon to reload (re-register backup capabilities, pick up
/// `/etc/bestool/backups` changes) without restarting.
pub async fn reload(addrs: &[std::net::SocketAddr]) -> miette::Result<()> {
	let (client, url) = try_connect_daemon(addrs).await?;
	let response = client
		.post(format!("{url}/reload"))
		.send()
		.await
		.map_err(|e| miette!("failed to request reload: {e}"))?;
	if !response.status().is_success() {
		return Err(miette!("reload endpoint returned {}", response.status()));
	}
	info!("connected to daemon at {url}");
	println!("Reload requested.");
	// The daemon stays up across a reload, so one check is enough.
	reprint_status(addrs, 1).await;
	Ok(())
}

/// Ask a running daemon to exit so the service manager restarts it (e.g. to pick
/// up a freshly-installed binary).
pub async fn restart(addrs: &[std::net::SocketAddr]) -> miette::Result<()> {
	let (client, url) = try_connect_daemon(addrs).await?;
	let response = client
		.post(format!("{url}/restart"))
		.send()
		.await
		.map_err(|e| miette!("failed to request restart: {e}"))?;
	if !response.status().is_success() {
		return Err(miette!("restart endpoint returned {}", response.status()));
	}
	info!("connected to daemon at {url}");
	println!("Restart requested; the service manager will bring the daemon back.");
	// The daemon is down until the service manager restarts it (the unit's
	// RestartSec is 10s) and it rebinds, so wait well past that before giving up.
	reprint_status(addrs, 20).await;
	Ok(())
}
