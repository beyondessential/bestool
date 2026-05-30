//! Shared HTTP readiness-probe helpers for the `tamanu` lifecycle
//! subcommands.
//!
//! `restart` probes each freshly-rolled instance until it responds; `start`
//! probes the behind-caddy services it brought up within a bounded budget.
//! The probe URL construction (container IP for systemd, pm2 PORT for pm2)
//! is shared so both commands target the same endpoint.

use std::time::Duration;

use jiff::SignedDuration;
use miette::{IntoDiagnostic, Result, bail};
use reqwest::{Client, Url};
use tracing::{debug, info, warn};

use bestool_tamanu::services::Supervisor;

use crate::actions::tamanu::lifecycle::{self, Instance};

pub fn http_client() -> Result<Client> {
	crate::http::client_builder()
		.timeout(Duration::from_secs(5))
		.build()
		.into_diagnostic()
}

pub fn parse_duration(s: &str) -> Result<Duration, String> {
	s.parse::<SignedDuration>()
		.map_err(|e| e.to_string())
		.and_then(|d| Duration::try_from(d).map_err(|e| e.to_string()))
}

/// Construct the readiness-probe URL for an instance, or `None` when one
/// can't be built (no container IP for systemd, no pm_id or no PORT for
/// pm2). Logs at warn/info the same way the callers expect.
pub fn instance_probe_url(supervisor: Supervisor, instance: &Instance) -> Result<Option<Url>> {
	let url = match supervisor {
		Supervisor::Systemd => {
			let unit = instance.unit();
			match lifecycle::container_ip_for_unit(&unit)? {
				Some(ip) => format!("http://{ip}:3000/").parse().into_diagnostic()?,
				None => {
					warn!(unit, "no container IP discovered, skipping HTTP probe");
					return Ok(None);
				}
			}
		}
		Supervisor::Pm2 => {
			let Some(pm_id) = instance.pm_id else {
				warn!(name = %instance.name, "pm2 instance has no pm_id, skipping HTTP probe");
				return Ok(None);
			};
			match lifecycle::pm2_port_for(pm_id)? {
				Some(port) => format!("http://127.0.0.1:{port}/").parse().into_diagnostic()?,
				None => {
					info!(name = %instance.name, pm_id, "no PORT in pm2 env, skipping HTTP probe");
					return Ok(None);
				}
			}
		}
	};
	Ok(Some(url))
}

/// Retry `url` every 500ms until it returns a non-5xx response. Never gives
/// up. Used for the per-instance readiness probe in the rolling restart,
/// where the container is guaranteed to come up (or the operator can ctrl+c).
pub async fn probe_until_ready(client: &Client, url: &Url) {
	loop {
		match probe_once(client, url).await {
			Ok(()) => {
				debug!(%url, "probe OK");
				return;
			}
			Err(err) => {
				debug!(%url, err = %err, "probe not ready, retrying");
				tokio::time::sleep(Duration::from_millis(500)).await;
			}
		}
	}
}

/// Bounded probe: retries with the same 500ms cadence but bails after
/// `timeout`. Used where a failure is an operator-facing result — the
/// post-restart `--check-url` end-to-end check and the post-start readiness
/// budget.
pub async fn probe_url(client: &Client, url: &Url, timeout: Duration) -> Result<()> {
	let deadline = std::time::Instant::now() + timeout;
	loop {
		match probe_once(client, url).await {
			Ok(()) => {
				debug!(%url, "probe OK");
				return Ok(());
			}
			Err(last_err) => {
				if std::time::Instant::now() >= deadline {
					bail!("HTTP probe of {url} failed: {last_err}");
				}
				debug!(%url, err = %last_err, "probe not ready, retrying");
				tokio::time::sleep(Duration::from_millis(500)).await;
			}
		}
	}
}

async fn probe_once(client: &Client, url: &Url) -> std::result::Result<(), String> {
	match client.get(url.clone()).send().await {
		Ok(resp) if !resp.status().is_server_error() => Ok(()),
		Ok(resp) => Err(format!("HTTP {}", resp.status())),
		Err(e) => Err(e.to_string()),
	}
}
