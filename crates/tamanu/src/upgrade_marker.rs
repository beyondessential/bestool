//! The rolling-upgrade marker file.
//!
//! An upgrade bumps the deployment's configured version first and then
//! restarts services one at a time, so a version-drift check that compares
//! running containers against the configured version fires by construction
//! for the whole rollout. The upgrade tooling writes this marker while it
//! works so the checks can tell mid-rollout drift from real drift.
//!
//! The marker lives on tmpfs, so a reboot clears it. A marker older than
//! [`FRESH_FOR`] is reported stale rather than honoured: an upgrade that has
//! been "in progress" that long has stalled, and the drift it was excusing is
//! worth alerting on again.

use std::{
	path::Path,
	time::{Duration, SystemTime},
};

/// Where the upgrade tooling writes the marker. Only Linux deployments have
/// one; elsewhere the path never exists and reads return `None`.
pub const MARKER_PATH: &str = "/run/tamanu/upgrade-in-progress";

/// How long a marker is honoured before it reads as a stalled rollout.
pub const FRESH_FOR: Duration = Duration::from_secs(90 * 60);

/// A read of the marker file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeMarker {
	/// An upgrade is underway; drift toward `target` is expected.
	Fresh {
		/// The version being rolled out, when the marker names one.
		target: Option<String>,
	},
	/// The marker has outlived [`FRESH_FOR`]: the rollout likely stalled.
	Stale {
		target: Option<String>,
		age: Duration,
	},
}

impl UpgradeMarker {
	pub fn is_fresh(&self) -> bool {
		matches!(self, Self::Fresh { .. })
	}

	pub fn target(&self) -> Option<&str> {
		match self {
			Self::Fresh { target } | Self::Stale { target, .. } => target.as_deref(),
		}
	}
}

/// Read the marker, `None` when there isn't one.
pub fn read() -> Option<UpgradeMarker> {
	read_at(Path::new(MARKER_PATH), SystemTime::now())
}

fn read_at(path: &Path, now: SystemTime) -> Option<UpgradeMarker> {
	let modified = std::fs::metadata(path).ok()?.modified().ok()?;
	let target = std::fs::read_to_string(path)
		.ok()
		.map(|content| content.trim().to_string())
		.filter(|content| !content.is_empty());
	// A modification time in the future reads as age zero, i.e. fresh.
	let age = now.duration_since(modified).unwrap_or_default();
	Some(if age <= FRESH_FOR {
		UpgradeMarker::Fresh { target }
	} else {
		UpgradeMarker::Stale { target, age }
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn no_marker_reads_as_none() {
		let tmp = tempfile::tempdir().unwrap();
		assert_eq!(read_at(&tmp.path().join("absent"), SystemTime::now()), None);
	}

	#[test]
	fn a_recent_marker_is_fresh_and_carries_the_target() {
		let tmp = tempfile::tempdir().unwrap();
		let marker = tmp.path().join("upgrade-in-progress");
		std::fs::write(&marker, "v2.61.5\n").unwrap();
		assert_eq!(
			read_at(&marker, SystemTime::now()),
			Some(UpgradeMarker::Fresh {
				target: Some("v2.61.5".into())
			})
		);
	}

	#[test]
	fn an_empty_marker_is_fresh_with_no_target() {
		let tmp = tempfile::tempdir().unwrap();
		let marker = tmp.path().join("upgrade-in-progress");
		std::fs::write(&marker, "").unwrap();
		assert_eq!(
			read_at(&marker, SystemTime::now()),
			Some(UpgradeMarker::Fresh { target: None })
		);
	}

	#[test]
	fn an_old_marker_is_stale() {
		let tmp = tempfile::tempdir().unwrap();
		let marker = tmp.path().join("upgrade-in-progress");
		std::fs::write(&marker, "v2.61.5").unwrap();
		let later = SystemTime::now() + FRESH_FOR + Duration::from_secs(60);
		let read = read_at(&marker, later).unwrap();
		assert!(!read.is_fresh(), "{read:?}");
		assert_eq!(read.target(), Some("v2.61.5"));
	}
}
