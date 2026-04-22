use std::net::IpAddr;

use miette::{IntoDiagnostic, Result, WrapErr};
use serde::Deserialize;
use tracing::trace;

/// Resolve `ip` to a Tailscale login name (e.g. `alice@example.com`) via the
/// local `tailscale whois --json` CLI. Returns `None` if the IP isn't known to
/// Tailscale, or if Tailscale isn't installed/running.
pub async fn whois(ip: &IpAddr) -> Result<Option<String>> {
	let ip = *ip;
	let output = tokio::task::spawn_blocking(move || {
		duct::cmd!("tailscale", "whois", "--json", ip.to_string())
			.stdout_capture()
			.stderr_capture()
			.unchecked()
			.run()
	})
	.await
	.into_diagnostic()?
	.into_diagnostic()
	.wrap_err("running tailscale whois")?;

	if !output.status.success() {
		trace!(
			status = ?output.status,
			stderr = %String::from_utf8_lossy(&output.stderr),
			"tailscale whois returned non-zero"
		);
		return Ok(None);
	}

	let parsed: WhoisJson = match serde_json::from_slice(&output.stdout) {
		Ok(p) => p,
		Err(err) => {
			trace!(?err, "failed to parse tailscale whois JSON");
			return Ok(None);
		}
	};

	Ok(parsed
		.user_profile
		.and_then(|u| u.login_name.or(u.display_name)))
}

#[derive(Debug, Deserialize)]
struct WhoisJson {
	#[serde(rename = "UserProfile")]
	user_profile: Option<UserProfile>,
}

#[derive(Debug, Deserialize)]
struct UserProfile {
	#[serde(rename = "LoginName")]
	login_name: Option<String>,
	#[serde(rename = "DisplayName")]
	display_name: Option<String>,
}
