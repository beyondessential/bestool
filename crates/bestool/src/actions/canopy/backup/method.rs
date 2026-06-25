//! Built-in backup methods.
//!
//! A backup def selects exactly one method. The driver runs the def's `pre`
//! hooks, calls [`Method::prepare`] to get a kopia source path (plus any
//! method-supplied tags), snapshots it, then calls [`Method::cleanup`] and the
//! `post` hooks. `type` is just the Canopy-facing label; the method is what
//! decides *how* to produce a consistent source.

use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
};

use miette::{Result, bail};
use serde::Deserialize;

/// A source ready for kopia to snapshot, produced by [`Method::prepare`].
#[derive(Debug)]
pub struct Prepared {
	/// The path kopia should snapshot.
	pub path: PathBuf,
	/// Extra tags the method contributes (merged with the canopy-* tags and the
	/// def's own `[tags]`).
	pub extra_tags: BTreeMap<String, String>,
	/// kopia ignore globs the driver applies to the source before snapshotting
	/// (e.g. postgres transient files).
	pub ignore: Vec<String>,
	/// Method-specific teardown, run by [`Method::cleanup`].
	pub(super) teardown: Teardown,
}

/// What [`Method::cleanup`] has to undo for a prepared source.
#[derive(Debug)]
pub(super) enum Teardown {
	/// The simple method's kopia-readable view (bindfs mount or copy).
	Simple(super::simple::Cleanup),
	/// A btrfs snapshot + its mounts.
	Btrfs(super::postgresql::btrfs::Mounts),
	/// A thin-LVM snapshot + its mount.
	Lvm(super::postgresql::lvm::Snapshot),
	/// A Windows VSS shadow copy to delete.
	Vss(super::postgresql::vss::Shadow),
	/// A streamed base backup directory to remove.
	BaseBackup(PathBuf),
}

/// `[simple]` method: snapshot a path verbatim.
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleConfig {
	/// The path kopia snapshots.
	pub path: PathBuf,
}

/// `[postgresql]` method: physical, crash-consistent cluster snapshot.
///
/// Driven entirely by this table — generic postgres, no Tamanu coupling.
#[derive(Debug, Clone, Deserialize)]
pub struct PostgresqlConfig {
	/// The cluster name (e.g. `main`); resolves the data dir / connection.
	pub cluster: String,
	/// Override the resolved data directory.
	#[serde(default)]
	pub data_dir: Option<PathBuf>,
	/// Override the resolved major version.
	#[serde(default)]
	pub version: Option<String>,
	/// Override the port used to connect for `CHECKPOINT`.
	#[serde(default)]
	pub port: Option<u16>,
	/// Override the unix socket directory used to connect.
	#[serde(default)]
	pub socket: Option<PathBuf>,
	/// Force a snapshot strategy instead of auto-detecting (for testing).
	#[serde(default)]
	pub strategy: Option<String>,
}

/// A built-in backup method, selected by the def's single method table.
#[derive(Debug, Clone)]
pub enum Method {
	Simple(SimpleConfig),
	Postgresql(PostgresqlConfig),
}

impl Method {
	/// The method's name, used in diagnostics.
	pub fn name(&self) -> &'static str {
		match self {
			Method::Simple(_) => "simple",
			Method::Postgresql(_) => "postgresql",
		}
	}

	/// Produce the source kopia will snapshot. `backup_type` is the def's label,
	/// used by methods that key stable paths on it (e.g. btrfs mount points).
	pub async fn prepare(&self, backup_type: &str) -> Result<Prepared> {
		match self {
			Method::Simple(config) => {
				let (path, cleanup) = super::simple::prepare(&config.path, backup_type).await?;
				Ok(Prepared {
					path,
					extra_tags: BTreeMap::new(),
					ignore: Vec::new(),
					teardown: Teardown::Simple(cleanup),
				})
			}
			Method::Postgresql(config) => super::postgresql::prepare(config, backup_type).await,
		}
	}

	/// Release whatever `prepare` set up (snapshot, mount, staging dir).
	pub async fn cleanup(&self, prepared: Prepared) -> Result<()> {
		match prepared.teardown {
			Teardown::Simple(cleanup) => super::simple::teardown(cleanup).await,
			Teardown::Btrfs(mounts) => super::postgresql::btrfs::teardown(mounts).await,
			Teardown::Lvm(snapshot) => super::postgresql::lvm::teardown(snapshot).await,
			Teardown::Vss(shadow) => super::postgresql::vss::teardown(shadow).await,
			Teardown::BaseBackup(root) => super::postgresql::basebackup::teardown(root).await,
		}
	}

	/// A staging directory for the restore, colocated with the eventual target's
	/// filesystem so the final move is an atomic rename. Falls back to the temp
	/// dir if the target can't be resolved.
	pub fn staging_dir(&self, target_override: Option<&Path>, pid: u32) -> PathBuf {
		let parent = match self {
			Method::Simple(config) => target_override
				.map(Path::to_path_buf)
				.unwrap_or_else(|| config.path.clone())
				.parent()
				.map(Path::to_path_buf),
			Method::Postgresql(config) => super::postgresql::resolve::resolve_target(config)
				.ok()
				.and_then(|r| r.data_dir.parent().map(Path::to_path_buf)),
		};
		parent
			.unwrap_or_else(std::env::temp_dir)
			.join(format!(".bestool-restore.{pid}"))
	}

	/// Lay a restored snapshot (in `staging`) back down. Method-specific: the
	/// simple method places files at its path; postgresql does the full
	/// stop/swap/start.
	pub async fn restore(&self, staging: &Path, opts: &RestoreOpts) -> Result<()> {
		match self {
			Method::Simple(config) => {
				let target = opts.target.clone().unwrap_or_else(|| config.path.clone());
				ensure_not_clobbering(&target, opts.clobber)?;
				replace_dir(staging, &target).await
			}
			Method::Postgresql(config) => super::postgresql::restore(config, staging, opts).await,
		}
	}
}

/// Options controlling a restore.
#[derive(Debug, Clone, Default)]
pub struct RestoreOpts {
	/// Override the destination (the simple method's path); ignored by postgresql,
	/// which always targets the configured cluster.
	pub target: Option<PathBuf>,
	/// Proceed even when the destination already holds data.
	pub clobber: bool,
}

/// Error unless `target` is safe to write (absent or empty) or `clobber` is set.
pub fn ensure_not_clobbering(target: &Path, clobber: bool) -> Result<()> {
	if clobber || !dir_has_entries(target) {
		return Ok(());
	}
	bail!(
		"{} already contains data; refusing to overwrite without confirmation \
		 (pass --clobber-existing-data-yes-i-am-sure, or confirm interactively)",
		target.display()
	);
}

/// Whether `path` is a directory with at least one entry.
pub fn dir_has_entries(path: &Path) -> bool {
	std::fs::read_dir(path)
		.map(|mut it| it.next().is_some())
		.unwrap_or(false)
}

/// Move `staging` into place at `target`, keeping any existing `target` as
/// `<target>.old`. Both must be on the same filesystem (atomic rename).
pub(super) async fn replace_dir(staging: &Path, target: &Path) -> Result<()> {
	use miette::{Context as _, IntoDiagnostic as _};

	if target.exists() {
		let backup = with_extension_suffix(target, "old");
		if backup.exists() {
			tokio::fs::remove_dir_all(&backup)
				.await
				.into_diagnostic()
				.wrap_err_with(|| format!("removing stale {}", backup.display()))?;
		}
		tokio::fs::rename(target, &backup)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("moving {} aside to {}", target.display(), backup.display()))?;
	}
	if let Some(parent) = target.parent() {
		tokio::fs::create_dir_all(parent).await.ok();
	}
	tokio::fs::rename(staging, target)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("moving restored data into {}", target.display()))
}

/// `/a/b` + `old` → `/a/b.old`.
pub(super) fn with_extension_suffix(path: &Path, suffix: &str) -> PathBuf {
	let mut name = path.file_name().unwrap_or_default().to_os_string();
	name.push(".");
	name.push(suffix);
	path.with_file_name(name)
}

#[cfg(test)]
mod tests {
	use super::*;

	// On Linux the simple method prepares a kopia-readable *view* (bindfs/copy),
	// which needs a real source, root, and the kopia user — so it's exercised
	// on-host, not here. Off Linux it still snapshots the path in place.
	#[cfg(not(target_os = "linux"))]
	#[tokio::test]
	async fn simple_prepare_returns_its_path_and_no_tags() {
		let method = Method::Simple(SimpleConfig {
			path: PathBuf::from("/data/custom"),
		});
		let prepared = method.prepare("custom").await.unwrap();
		assert_eq!(prepared.path, PathBuf::from("/data/custom"));
		assert!(prepared.extra_tags.is_empty());
		assert!(prepared.ignore.is_empty());
		method.cleanup(prepared).await.unwrap();
	}

	#[test]
	fn clobber_guard_blocks_occupied_dir_unless_forced() {
		let tmp = tempfile::tempdir().unwrap();
		let occupied = tmp.path().join("data");
		std::fs::create_dir_all(&occupied).unwrap();
		std::fs::write(occupied.join("PG_VERSION"), "16").unwrap();

		assert!(ensure_not_clobbering(&occupied, false).is_err());
		assert!(ensure_not_clobbering(&occupied, true).is_ok());

		// Absent or empty targets are fine without forcing.
		assert!(ensure_not_clobbering(&tmp.path().join("absent"), false).is_ok());
		let empty = tmp.path().join("empty");
		std::fs::create_dir_all(&empty).unwrap();
		assert!(ensure_not_clobbering(&empty, false).is_ok());
	}

	#[test]
	fn extension_suffix_appends() {
		assert_eq!(
			with_extension_suffix(Path::new("/var/lib/postgresql/16/main"), "old"),
			PathBuf::from("/var/lib/postgresql/16/main.old")
		);
	}

	#[tokio::test]
	async fn replace_dir_keeps_old_and_moves_in() {
		let tmp = tempfile::tempdir().unwrap();
		let target = tmp.path().join("data");
		std::fs::create_dir_all(&target).unwrap();
		std::fs::write(target.join("old-marker"), "x").unwrap();
		let staging = tmp.path().join("staging");
		std::fs::create_dir_all(&staging).unwrap();
		std::fs::write(staging.join("new-marker"), "y").unwrap();

		replace_dir(&staging, &target).await.unwrap();

		assert!(target.join("new-marker").exists());
		assert!(!target.join("old-marker").exists());
		assert!(tmp.path().join("data.old").join("old-marker").exists());
	}
}
