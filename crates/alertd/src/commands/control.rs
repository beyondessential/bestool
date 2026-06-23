//! Clients for the daemon's `/reload` and `/restart` control endpoints.

use miette::miette;
use tracing::info;

use super::try_connect_daemon;

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
	Ok(())
}
