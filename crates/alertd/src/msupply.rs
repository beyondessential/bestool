//! The mSupply application on this machine.
//!
//! spec: SUBJ

use std::path::Path;

/// The application type as canopy names it.
pub const TYPE_SLUG: &str = "msupply";

/// The Podman unit whose presence says mSupply is installed here.
pub const CONTAINER_UNIT: &str = "/etc/containers/systemd/msupply.container";

/// The environment file that pins the installed version.
pub const ENV_FILE: &str = "/etc/msupply/env";

/// Whether mSupply is installed on this machine.
pub fn installed() -> bool {
	Path::new(CONTAINER_UNIT).is_file()
}

/// The installed product version, read from the version the installation pins.
pub fn version() -> Option<String> {
	version_from(&std::fs::read_to_string(ENV_FILE).ok()?)
}

/// The product version in an environment file's `MSUPPLY_VERSION`, which is
/// the image tag: `v2.17.06-sqlite-amd64` is version `2.17.06`.
pub fn version_from(env: &str) -> Option<String> {
	let tag = env.lines().find_map(|line| {
		let value = line.trim().strip_prefix("MSUPPLY_VERSION=")?;
		Some(value.trim().trim_matches(['"', '\'']).to_owned())
	})?;
	let version = tag.strip_prefix('v').unwrap_or(&tag);
	let version = version.split('-').next().unwrap_or(version);
	(!version.is_empty()).then(|| version.to_owned())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_version_is_the_tag_without_its_prefix_and_variant() {
		assert_eq!(
			version_from("MSUPPLY_VERSION=v2.17.06-sqlite-amd64\n").as_deref(),
			Some("2.17.06")
		);
		assert_eq!(
			version_from("OTHER=1\nMSUPPLY_VERSION=\"2.3.0\"\n").as_deref(),
			Some("2.3.0")
		);
	}

	#[test]
	fn no_pinned_version_is_no_version() {
		assert_eq!(version_from("OTHER=1\n"), None);
		assert_eq!(version_from("MSUPPLY_VERSION=\n"), None);
	}
}
