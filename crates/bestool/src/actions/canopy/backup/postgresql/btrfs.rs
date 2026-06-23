//! Crash-consistent btrfs snapshot of a postgres cluster.
//!
//! Mirrors the proven `kopia-backup-postgres-btrfs.sh` approach: take an atomic,
//! read-only btrfs snapshot of the subvolume the data directory lives on (which
//! includes `pg_wal`), mount it read-only at a **stable** path (so kopia's
//! snapshot history/dedup attribute to one source), and hand kopia the cluster
//! directory within. No `pg_backup_start`/`backup_label` — the snapshot restores
//! by plain crash recovery.
//!
//! The privileged steps (mount, `btrfs subvolume snapshot`) can't run in unit
//! tests; the pure helpers (names, paths) are tested and the whole flow is
//! verified on a real btrfs host per the plan.

use std::path::{Path, PathBuf};

use miette::{Result, miette};
use tracing::{info, warn};

use super::{resolve::ResolvedCluster, sys};

/// Where the reaper looks for / makes per-run top-level mounts.
const TOPLEVEL_MOUNT_PREFIX: &str = "/mnt/bestool-btrfs-toplevel";

/// Infix marking our ephemeral snapshot subvolumes, so the reaper's glob can
/// never match a live subvolume.
const SNAPSHOT_INFIX: &str = "bestool-kopia-";

/// Teardown state for a prepared btrfs snapshot, released by [`teardown`].
#[derive(Debug)]
pub struct Mounts {
	/// The top-level (subvolid=5) mount the snapshot subvolume lives under.
	toplevel_mount: PathBuf,
	/// The ephemeral snapshot subvolume's path (under `toplevel_mount`).
	snapshot_path: PathBuf,
	/// The stable read-only mount kopia reads from.
	kopia_mount: PathBuf,
}

/// The stable mount path for a backup type (see [`super::stable_source_dir`]).
fn stable_kopia_mount(backup_type: &str) -> PathBuf {
	super::stable_source_dir(backup_type)
}

/// Name for this run's ephemeral snapshot subvolume.
fn snapshot_name(pid: u32) -> String {
	format!("{SNAPSHOT_INFIX}{pid}")
}

/// This run's top-level mount path.
fn toplevel_mount(pid: u32) -> PathBuf {
	PathBuf::from(format!("{TOPLEVEL_MOUNT_PREFIX}.{pid}"))
}

/// Take the snapshot and mount it; returns the kopia source path and the
/// teardown state. Caller must always pass the result to [`teardown`].
pub async fn prepare(resolved: &ResolvedCluster, backup_type: &str) -> Result<(PathBuf, Mounts)> {
	let pid = std::process::id();
	let base_mount = sys::findmnt_target(&resolved.data_dir).await?;
	let rel = sys::relative_data_path(&resolved.data_dir, &base_mount)?;
	let fsdev = format!(
		"/dev/disk/by-uuid/{}",
		sys::findmnt_field("UUID", &resolved.data_dir).await?
	);
	let map = sys::postgres_to_kopia_idmap().await?;

	let kopia_mount = stable_kopia_mount(backup_type);
	reap_stale(&fsdev, &kopia_mount).await;

	let toplevel_mount = toplevel_mount(pid);
	let snapshot_name = snapshot_name(pid);
	let snapshot_path = toplevel_mount.join(&snapshot_name);

	// Build the teardown state up front so a failure mid-prepare still cleans up.
	let mut mounts = Mounts {
		toplevel_mount: toplevel_mount.clone(),
		snapshot_path: PathBuf::new(),
		kopia_mount: PathBuf::new(),
	};

	sys::mkdir(&toplevel_mount).await?;
	sys::run_ok(
		"mount",
		&["-o", "subvolid=5", &fsdev, sys::path(&toplevel_mount)],
	)
	.await?;

	info!(snapshot = %snapshot_path.display(), "creating read-only btrfs snapshot");
	sys::run_ok(
		"btrfs",
		&[
			"subvolume",
			"snapshot",
			"-r",
			sys::path(&base_mount),
			sys::path(&snapshot_path),
		],
	)
	.await?;
	mounts.snapshot_path = snapshot_path.clone();

	sys::mkdir(&kopia_mount).await?;
	sys::run_ok(
		"mount",
		&[
			&fsdev,
			sys::path(&kopia_mount),
			"-o",
			&format!("subvol={snapshot_name},X-mount.idmap={map}"),
		],
	)
	.await?;
	mounts.kopia_mount = kopia_mount.clone();

	Ok((kopia_mount.join(rel), mounts))
}

/// Release a prepared snapshot: unmount the kopia mount, delete the snapshot
/// subvolume, unmount and remove the top-level mount. Best-effort throughout.
pub async fn teardown(mounts: Mounts) -> Result<()> {
	if !mounts.kopia_mount.as_os_str().is_empty() {
		sys::umount(&mounts.kopia_mount).await;
		sys::rmdir(&mounts.kopia_mount).await;
	}
	if !mounts.snapshot_path.as_os_str().is_empty() {
		let _ = sys::run_ok(
			"btrfs",
			&["subvolume", "delete", sys::path(&mounts.snapshot_path)],
		)
		.await
		.map_err(|err| miette!("deleting snapshot subvolume: {err}"))
		.inspect_err(|err| warn!("{err}"));
	}
	if !mounts.toplevel_mount.as_os_str().is_empty() {
		sys::umount(&mounts.toplevel_mount).await;
		sys::rmdir(&mounts.toplevel_mount).await;
	}
	Ok(())
}

/// Sweep leftovers from a previously crashed run (hard reboot skips teardown):
/// the stable kopia mount, stray top-level mounts, and orphaned `bestool-kopia-*`
/// snapshot subvolumes. All best-effort.
async fn reap_stale(fsdev: &str, kopia_mount: &Path) {
	sys::umount(kopia_mount).await;

	if let Ok(entries) = sys::glob_prefix("/mnt", "bestool-btrfs-toplevel.") {
		for stale in entries {
			sys::umount(&stale).await;
			sys::rmdir(&stale).await;
		}
	}

	let reap_mount = PathBuf::from(format!("{TOPLEVEL_MOUNT_PREFIX}-reap.{}", std::process::id()));
	if sys::mkdir(&reap_mount).await.is_ok()
		&& sys::run_ok("mount", &["-o", "subvolid=5", fsdev, sys::path(&reap_mount)])
			.await
			.is_ok()
	{
		if let Ok(subs) = sys::glob_prefix(sys::path(&reap_mount), SNAPSHOT_INFIX) {
			for sub in subs {
				let _ = sys::run_ok("btrfs", &["subvolume", "delete", sys::path(&sub)]).await;
			}
		}
		sys::umount(&reap_mount).await;
	}
	sys::rmdir(&reap_mount).await;
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn snapshot_name_carries_reaper_infix() {
		let name = snapshot_name(4242);
		assert_eq!(name, "bestool-kopia-4242");
		assert!(name.starts_with(SNAPSHOT_INFIX));
	}

	#[test]
	fn stable_mount_is_per_type_and_fixed() {
		assert_eq!(
			stable_kopia_mount("tamanu-postgres"),
			PathBuf::from("/var/lib/kopia/bestool-backup/tamanu-postgres")
		);
	}

	#[test]
	fn toplevel_mount_path() {
		assert_eq!(
			toplevel_mount(7),
			PathBuf::from("/mnt/bestool-btrfs-toplevel.7")
		);
	}
}
