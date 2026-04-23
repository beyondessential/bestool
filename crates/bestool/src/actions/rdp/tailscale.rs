use chrono::{DateTime, Utc};
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::Deserialize;
use tracing::trace;

/// Resolve `addr` to a Tailscale login name (e.g. `alice@example.com`) via the
/// local `tailscale whois --json` CLI. Returns `None` if the address isn't
/// known to Tailscale, or if Tailscale isn't installed/running.
///
/// Any `%<zone>` suffix (Windows IPv6 scope identifier) is stripped before
/// calling the CLI.
pub async fn whois(addr: &str) -> Result<Option<String>> {
	let stripped = addr.split('%').next().unwrap_or(addr).to_owned();
	let output = tokio::task::spawn_blocking(move || {
		duct::cmd!("tailscale", "whois", "--json", &stripped)
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

/// A Tailscale peer with a recent wireguard handshake, as reported by
/// `tailscale status --json`.
#[derive(Debug, Clone)]
pub struct ActivePeer {
	pub login: String,
	pub host_name: String,
	pub last_handshake: DateTime<Utc>,
}

/// Return all currently-active Tailscale peers (excluding self), ordered from
/// most-recent handshake to least. Used as a fallback when an RDP event's
/// `Address` field is present but isn't a resolvable client IP (common when
/// Windows logs the Tailscale interface's local endpoint for IPv6 connections).
pub async fn active_peers() -> Result<Vec<ActivePeer>> {
	let output = tokio::task::spawn_blocking(|| {
		duct::cmd!("tailscale", "status", "--json")
			.stdout_capture()
			.stderr_capture()
			.unchecked()
			.run()
	})
	.await
	.into_diagnostic()?
	.into_diagnostic()
	.wrap_err("running tailscale status")?;

	if !output.status.success() {
		trace!(
			status = ?output.status,
			stderr = %String::from_utf8_lossy(&output.stderr),
			"tailscale status returned non-zero"
		);
		return Ok(Vec::new());
	}

	parse_status(&output.stdout)
}

fn parse_status(bytes: &[u8]) -> Result<Vec<ActivePeer>> {
	let status: StatusJson = match serde_json::from_slice(bytes) {
		Ok(s) => s,
		Err(err) => {
			trace!(?err, "failed to parse tailscale status JSON");
			return Ok(Vec::new());
		}
	};

	let mut peers: Vec<ActivePeer> = status
		.peer
		.unwrap_or_default()
		.into_values()
		.filter_map(|p| {
			let handshake = p.last_handshake.and_then(parse_handshake)?;
			let login = status
				.user
				.as_ref()
				.and_then(|users| users.get(&p.user_id?))
				.and_then(|u| u.login_name.clone().or_else(|| u.display_name.clone()))?;
			Some(ActivePeer {
				login,
				host_name: p.host_name.unwrap_or_default(),
				last_handshake: handshake,
			})
		})
		.collect();
	peers.sort_by_key(|p| std::cmp::Reverse(p.last_handshake));
	Ok(peers)
}

/// Tailscale emits a zero-time sentinel (`0001-01-01T00:00:00Z`) for peers that
/// have never handshook. Filter those out so they don't show up as "most
/// recent".
fn parse_handshake(s: String) -> Option<DateTime<Utc>> {
	let parsed: DateTime<Utc> = s.parse().ok()?;
	if parsed.timestamp() <= 0 {
		None
	} else {
		Some(parsed)
	}
}

#[derive(Debug, Deserialize)]
struct StatusJson {
	#[serde(rename = "Peer")]
	peer: Option<std::collections::HashMap<String, PeerJson>>,
	#[serde(rename = "User")]
	user: Option<std::collections::HashMap<u64, UserProfile>>,
}

#[derive(Debug, Deserialize)]
struct PeerJson {
	#[serde(rename = "HostName")]
	host_name: Option<String>,
	#[serde(rename = "UserID")]
	user_id: Option<u64>,
	#[serde(rename = "LastHandshake")]
	last_handshake: Option<String>,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_status_and_sorts_by_handshake() {
		let json = br#"{
			"Peer": {
				"AAAA": {
					"HostName": "stale",
					"UserID": 1,
					"LastHandshake": "0001-01-01T00:00:00Z"
				},
				"BBBB": {
					"HostName": "laptop",
					"UserID": 1,
					"LastHandshake": "2026-04-22T23:23:00Z"
				},
				"CCCC": {
					"HostName": "phone",
					"UserID": 2,
					"LastHandshake": "2026-04-22T23:20:00Z"
				}
			},
			"User": {
				"1": { "LoginName": "alice@bes.au" },
				"2": { "LoginName": "bob@bes.au" }
			}
		}"#;
		let peers = parse_status(json).unwrap();
		assert_eq!(peers.len(), 2);
		assert_eq!(peers[0].login, "alice@bes.au");
		assert_eq!(peers[0].host_name, "laptop");
		assert_eq!(peers[1].login, "bob@bes.au");
	}

	#[test]
	fn empty_status_gives_empty_list() {
		let json = br#"{"Peer": {}, "User": {}}"#;
		assert!(parse_status(json).unwrap().is_empty());
	}
}
