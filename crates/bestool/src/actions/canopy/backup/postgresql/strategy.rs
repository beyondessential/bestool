//! Pick the snapshot strategy for a cluster's storage backend.
//!
//! btrfs is implemented here; thin-LVM, Windows VSS, and the `pg_basebackup`
//! fallback land in follow-up PRs. Detection walks from the data directory to
//! its filesystem type; an explicit `strategy =` in the config overrides it (for
//! testing).

use std::{path::Path, process::Command};

use miette::{Result, bail};

/// How a cluster's data directory is captured for backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
	/// Crash-consistent btrfs subvolume snapshot.
	Btrfs,
	/// Crash-consistent thin-LVM snapshot — not yet implemented.
	ThinLvm,
	/// Crash-consistent Windows VSS shadow copy — not yet implemented.
	Vss,
	/// `pg_basebackup` base backup — not yet implemented.
	BaseBackup,
}

impl Strategy {
	fn parse(name: &str) -> Result<Self> {
		Ok(match name {
			"btrfs" => Strategy::Btrfs,
			"thin-lvm" | "thinlvm" | "lvm" => Strategy::ThinLvm,
			"vss" => Strategy::Vss,
			"basebackup" | "base-backup" | "pg_basebackup" => Strategy::BaseBackup,
			other => bail!("unknown backup strategy override '{other}'"),
		})
	}
}

/// Map a filesystem type to its strategy.
///
/// btrfs takes the cheap snapshot path; everything else (thick LV, ext4/xfs,
/// …) falls back to `pg_basebackup`, which is always correct. (Thin-LVM
/// detection — distinguishing it from thick — is a separate step added with the
/// thin-LVM strategy.)
fn fstype_to_strategy(fstype: &str) -> Strategy {
	match fstype {
		"btrfs" => Strategy::Btrfs,
		_ => Strategy::BaseBackup,
	}
}

/// Detect the strategy for `data_dir`, honouring a config override.
pub fn detect(strategy_override: Option<&str>, data_dir: &Path) -> Result<Strategy> {
	if let Some(name) = strategy_override {
		return Strategy::parse(name);
	}
	if cfg!(windows) {
		return Ok(Strategy::Vss);
	}
	let fstype = findmnt_fstype(data_dir)?;
	Ok(fstype_to_strategy(&fstype))
}

/// `findmnt -no FSTYPE --target <path>` — the filesystem type backing `path`.
fn findmnt_fstype(path: &Path) -> Result<String> {
	let output = Command::new("findmnt")
		.args(["-no", "FSTYPE", "--target"])
		.arg(path)
		.output()
		.map_err(|e| miette::miette!("running findmnt for {}: {e}", path.display()))?;
	if !output.status.success() {
		bail!(
			"findmnt failed for {}: {}",
			path.display(),
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn fstype_mapping() {
		assert_eq!(fstype_to_strategy("btrfs"), Strategy::Btrfs);
		assert_eq!(fstype_to_strategy("ext4"), Strategy::BaseBackup);
		assert_eq!(fstype_to_strategy("xfs"), Strategy::BaseBackup);
	}

	#[test]
	fn override_parsing() {
		assert_eq!(Strategy::parse("btrfs").unwrap(), Strategy::Btrfs);
		assert_eq!(Strategy::parse("basebackup").unwrap(), Strategy::BaseBackup);
		assert!(Strategy::parse("nonsense").is_err());
	}

	#[test]
	fn override_takes_precedence() {
		// A bogus path is never touched when the override is set.
		assert_eq!(
			detect(Some("btrfs"), Path::new("/nonexistent")).unwrap(),
			Strategy::Btrfs
		);
	}
}
