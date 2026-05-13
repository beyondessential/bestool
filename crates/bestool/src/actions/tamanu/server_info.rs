//! Shared helpers for collecting Tamanu server identity and host facts.
//!
//! Used by `meta_ticket`, `alertd`, and `doctor`. Kept here so each subcommand
//! pulls from one place rather than reaching into a sibling's module.

use std::process::Command;

use miette::{IntoDiagnostic, Result};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Look up the `metaServerId` in `local_system_facts`, creating one if absent.
///
/// The ID is a UUIDv4 and is stable for the lifetime of the database.
pub async fn get_or_create_server_id(client: &tokio_postgres::Client) -> Result<String> {
	let row = client
		.query_opt(
			"SELECT value FROM local_system_facts WHERE key = 'metaServerId'",
			&[],
		)
		.await
		.into_diagnostic()?;

	if let Some(row) = row {
		let id: String = row.try_get(0).into_diagnostic()?;
		debug!(server_id = %id, "found existing metaServerId");
		return Ok(id);
	}

	let id = Uuid::new_v4().to_string();
	info!(server_id = %id, "generating new metaServerId");
	client
		.execute(
			"INSERT INTO local_system_facts (key, value) VALUES ('metaServerId', $1)",
			&[&id],
		)
		.await
		.into_diagnostic()?;

	Ok(id)
}

/// Best-effort fetch of the Tamanu deviceKey PEM from `local_system_facts`.
///
/// Returns `None` if the DB is unreachable or the row is missing. Logging is
/// the only signal: callers without a device key continue to work (canopy
/// tailscale path is still available).
pub async fn fetch_device_key(database_url: &str) -> Option<String> {
	let (client, connection) = match tokio_postgres::connect(database_url, tokio_postgres::NoTls)
		.await
	{
		Ok(c) => c,
		Err(err) => {
			warn!("failed to connect for deviceKey fetch: {err}");
			return None;
		}
	};
	tokio::spawn(async move {
		if let Err(err) = connection.await {
			warn!("deviceKey-fetch connection error: {err}");
		}
	});

	match client
		.query_opt(
			"SELECT value FROM local_system_facts WHERE key = 'deviceKey'",
			&[],
		)
		.await
	{
		Ok(Some(row)) => match row.try_get::<_, String>(0) {
			Ok(pem) => {
				info!("loaded deviceKey from Tamanu DB for canopy targets");
				Some(pem)
			}
			Err(err) => {
				warn!("deviceKey row not a string: {err}");
				None
			}
		},
		Ok(None) => {
			info!("no deviceKey in local_system_facts; canopy targets unavailable");
			None
		}
		Err(err) => {
			warn!("failed to query deviceKey: {err}");
			None
		}
	}
}

/// Read the tailscale Self node's first IP and DNS name, if tailscale is
/// installed and responding to `status --json`.
pub fn get_tailscale_info() -> (Option<String>, Option<String>) {
	let output = match Command::new("tailscale")
		.arg("status")
		.arg("--json")
		.output()
	{
		Ok(output) if output.status.success() => output,
		Ok(output) => {
			debug!(
				status = %output.status,
				"tailscale status command failed"
			);
			return (None, None);
		}
		Err(e) => {
			debug!(error = %e, "tailscale not available");
			return (None, None);
		}
	};

	let parsed: serde_json::Value = match serde_json::from_slice(&output.stdout) {
		Ok(v) => v,
		Err(e) => {
			warn!(error = %e, "failed to parse tailscale status JSON");
			return (None, None);
		}
	};

	let self_node = &parsed["Self"];
	let ip = self_node["TailscaleIPs"]
		.as_array()
		.and_then(|ips| ips.first())
		.and_then(|ip| ip.as_str())
		.map(String::from);
	let name = self_node["DNSName"]
		.as_str()
		.map(|s| s.trim_end_matches('.').to_string());

	(ip, name)
}

/// Linux: read `systemd-detect-virt`'s output. Returns `None` if the command
/// is unavailable. The string is whatever systemd reports (e.g. `kvm`, `lxc`,
/// `none` for bare metal).
pub fn detect_virtualisation() -> Option<String> {
	let output = Command::new("systemd-detect-virt").output().ok()?;

	let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
	if stdout.is_empty() {
		return None;
	}

	Some(stdout)
}

#[cfg(test)]
mod tests {
	use std::sync::Mutex;

	use tokio_postgres::NoTls;

	use super::*;

	static DB_TEST_MUTEX: Mutex<()> = Mutex::new(());

	async fn test_db_client() -> tokio_postgres::Client {
		let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
		let (client, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
		tokio::spawn(async move {
			if let Err(e) = connection.await {
				eprintln!("connection error: {e}");
			}
		});

		client
			.batch_execute(
				"CREATE TABLE IF NOT EXISTS local_system_facts (
					key TEXT PRIMARY KEY,
					value TEXT NOT NULL
				)",
			)
			.await
			.unwrap();
		client
			.execute(
				"DELETE FROM local_system_facts WHERE key = 'metaServerId'",
				&[],
			)
			.await
			.unwrap();

		client
	}

	#[tokio::test]
	async fn test_get_or_create_server_id_generates_new() {
		let _lock = DB_TEST_MUTEX.lock().unwrap();
		let client = test_db_client().await;

		let id = get_or_create_server_id(&client).await.unwrap();
		assert!(!id.is_empty());
		uuid::Uuid::parse_str(&id).expect("should be a valid UUID");

		let row = client
			.query_one(
				"SELECT value FROM local_system_facts WHERE key = 'metaServerId'",
				&[],
			)
			.await
			.unwrap();
		let stored: String = row.get(0);
		assert_eq!(stored, id);
	}

	#[tokio::test]
	async fn test_get_or_create_server_id_returns_existing() {
		let _lock = DB_TEST_MUTEX.lock().unwrap();
		let client = test_db_client().await;

		client
			.execute(
				"INSERT INTO local_system_facts (key, value) VALUES ('metaServerId', 'existing-id-123')",
				&[],
			)
			.await
			.unwrap();

		let id = get_or_create_server_id(&client).await.unwrap();
		assert_eq!(id, "existing-id-123");
	}

	#[tokio::test]
	async fn test_get_or_create_server_id_is_stable() {
		let _lock = DB_TEST_MUTEX.lock().unwrap();
		let client = test_db_client().await;

		let id1 = get_or_create_server_id(&client).await.unwrap();
		let id2 = get_or_create_server_id(&client).await.unwrap();
		assert_eq!(id1, id2);
	}
}
