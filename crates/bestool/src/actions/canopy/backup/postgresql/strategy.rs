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

/// Whether an `lvs -o segtype` value is a thin LV (the only LVM kind that gets
/// a cheap snapshot; thick LVs fall back to `pg_basebackup`).
fn segtype_is_thin(segtype: &str) -> bool {
	segtype.trim() == "thin"
}

/// Detect the strategy for `data_dir`, honouring a config override.
///
/// btrfs and thin-LVM get cheap crash-consistent snapshots; everything else
/// (thick LV, plain ext4/xfs partition) falls back to `pg_basebackup`, which is
/// always correct.
pub fn detect(strategy_override: Option<&str>, data_dir: &Path) -> Result<Strategy> {
	if let Some(name) = strategy_override {
		return Strategy::parse(name);
	}
	if cfg!(windows) {
		return Ok(Strategy::Vss);
	}
	if findmnt_field("FSTYPE", data_dir)? == "btrfs" {
		return Ok(Strategy::Btrfs);
	}
	if is_thin_lvm(data_dir) {
		return Ok(Strategy::ThinLvm);
	}
	Ok(Strategy::BaseBackup)
}

/// Whether `data_dir`'s backing device is a thin LV. Any failure (not LVM, no
/// `lvs`) is treated as "not thin" — the `pg_basebackup` fallback is correct.
fn is_thin_lvm(data_dir: &Path) -> bool {
	let Ok(source) = findmnt_field("SOURCE", data_dir) else {
		return false;
	};
	let Ok(output) = Command::new("lvs")
		.args(["--noheadings", "-o", "segtype", &source])
		.output()
	else {
		return false;
	};
	output.status.success() && segtype_is_thin(&String::from_utf8_lossy(&output.stdout))
}

/// A single `findmnt -no <field> --target <path>` value.
fn findmnt_field(field: &str, path: &Path) -> Result<String> {
	let output = Command::new("findmnt")
		.args(["-no", field, "--target"])
		.arg(path)
		.output()
		.map_err(|e| miette::miette!("running findmnt for {}: {e}", path.display()))?;
	if !output.status.success() {
		bail!(
			"findmnt {field} failed for {}: {}",
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
	fn thin_segtype_detection() {
		assert!(segtype_is_thin("thin"));
		assert!(segtype_is_thin("  thin  "));
		assert!(!segtype_is_thin("linear"));
		assert!(!segtype_is_thin("striped"));
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
