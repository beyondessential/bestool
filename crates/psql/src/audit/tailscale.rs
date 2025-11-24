use std::process::Command;

use miette::{IntoDiagnostic as _, Result};
use serde::{Deserialize, Serialize};

/// Information about a Tailscale peer
#[derive(Debug, Clone, Serialize, Deserialize, facet::Facet)]
pub struct TailscalePeer {
	/// Device hostname
	pub device: String,
	/// User login name
	pub user: String,
}

/// Get active Tailscale human peers
pub fn get_active_peers() -> Result<Vec<TailscalePeer>> {
	let output = Command::new("tailscale")
		.arg("status")
		.arg("--json")
		.output()
		.into_diagnostic()?;

	if !output.status.success() {
		return Err(miette::miette!("tailscale command failed"));
	}

	let json: serde_json::Value = serde_json::from_slice(&output.stdout).into_diagnostic()?;

	let mut peers = Vec::new();

	let user_map = json.get("User").and_then(|u| u.as_object());

	if let Some(peer_map) = json.get("Peer").and_then(|p| p.as_object()) {
		for (_key, peer) in peer_map {
			let active = peer
				.get("Active")
				.and_then(|a| a.as_bool())
				.unwrap_or(false);

			if !active {
				continue;
			}

			let has_tags = peer
				.get("Tags")
				.and_then(|t| t.as_array())
				.map(|arr| !arr.is_empty())
				.unwrap_or(false);

			if has_tags {
				continue;
			}

			let device = peer
				.get("HostName")
				.and_then(|h| h.as_str())
				.map(|s| s.to_string());

			let user_id = peer.get("UserID").and_then(|id| id.as_u64());

			let user = if let (Some(user_map), Some(user_id)) = (user_map, user_id) {
				user_map
					.get(&user_id.to_string())
					.and_then(|u| u.get("LoginName"))
					.and_then(|l| l.as_str())
					.map(|s| s.to_string())
			} else {
				None
			};

			if let (Some(device), Some(user)) = (device, user) {
				peers.push(TailscalePeer { device, user });
			}
		}
	}

	if peers.is_empty() {
		Err(miette::miette!("no active tailscale peers found"))
	} else {
		Ok(peers)
	}
}
