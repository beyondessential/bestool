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

use jiff::Timestamp;
use miette::{Context as _, IntoDiagnostic as _, Result, miette};
use tracing::{info, warn};

use super::{
	super::hold::{HeldCapture, release},
	resolve::ResolvedCluster,
	sys,
};

/// Directory the per-run top-level mounts live in: the daemon's root-owned
/// StateDirectory. Not `/mnt` (read-only under `ProtectSystem=strict`) nor
/// `/var/lib/kopia` (the kopia user's home, which root can't write).
const TOPLEVEL_MOUNT_DIR: &str = "/var/lib/bestool";

/// Where the reaper looks for / makes per-run top-level mounts.
const TOPLEVEL_MOUNT_PREFIX: &str = "/var/lib/bestool/bestool-btrfs-toplevel";

/// Infix marking our ephemeral snapshot subvolumes, so the reaper's glob can
/// never match a live subvolume.
const SNAPSHOT_INFIX: &str = "bestool-kopia-";

/// Infix marking a snapshot subvolume promoted to a hold. Deliberately not
/// [`SNAPSHOT_INFIX`]: the reaper deletes everything carrying that one, so a held
/// capture left under it would be destroyed by the next run of any type.
const HELD_INFIX: &str = "bestool-held-";

/// Where a held capture's top-level mount lives — likewise outside the prefix
/// [`reap_stale`] sweeps.
const HELD_TOPLEVEL_PREFIX: &str = "/var/lib/bestool/bestool-btrfs-held";

/// Teardown state for a prepared btrfs snapshot, released by [`teardown`].
#[derive(Debug)]
pub struct Mounts {
	/// The top-level (subvolid=5) mount the snapshot subvolume lives under.
	toplevel_mount: PathBuf,
	/// The ephemeral snapshot subvolume's path (under `toplevel_mount`).
	snapshot_path: PathBuf,
	/// The stable read-only mount kopia reads from.
	kopia_mount: PathBuf,
	/// The filesystem's device path, for mounting the capture again elsewhere.
	fsdev: String,
	/// The postgres-to-kopia id map the kopia mount was made with.
	idmap: String,
}

/// The stable mount path for a backup type (see [`super::stable_source_dir`]).
fn stable_kopia_mount(backup_type: &str) -> PathBuf {
	super::stable_source_dir(backup_type)
}

/// Name for this run's ephemeral snapshot subvolume.
fn snapshot_name(token: &str) -> String {
	format!("{SNAPSHOT_INFIX}{token}")
}

/// This run's top-level mount path.
fn toplevel_mount(token: &str) -> PathBuf {
	PathBuf::from(format!("{TOPLEVEL_MOUNT_PREFIX}.{token}"))
}

/// Take the snapshot and mount it; returns the kopia source path and the
/// teardown state. Caller must always pass the result to [`teardown`].
pub async fn prepare(
	resolved: &ResolvedCluster,
	backup_type: &str,
) -> Result<(PathBuf, Timestamp, Mounts)> {
	let token = sys::run_token();
	let base_mount = sys::findmnt_target(&resolved.data_dir).await?;
	let rel = sys::relative_data_path(&resolved.data_dir, &base_mount)?;
	let fsdev = format!(
		"/dev/disk/by-uuid/{}",
		sys::findmnt_field("UUID", &resolved.data_dir).await?
	);
	let map = sys::postgres_to_kopia_idmap().await?;

	let kopia_mount = stable_kopia_mount(backup_type);
	reap_stale(&fsdev, &kopia_mount).await;

	let toplevel_mount = toplevel_mount(&token);
	let snapshot_name = snapshot_name(&token);
	let snapshot_path = toplevel_mount.join(&snapshot_name);

	// Build the teardown state up front so a failure mid-prepare still cleans up.
	let mut mounts = Mounts {
		toplevel_mount: toplevel_mount.clone(),
		snapshot_path: PathBuf::new(),
		kopia_mount: PathBuf::new(),
		fsdev: fsdev.clone(),
		idmap: map.clone(),
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
	// The read-only snapshot now exists: this is the instant the data froze.
	let taken_at = Timestamp::now();
	mounts.snapshot_path = snapshot_path.clone();

	sys::mkdir(&kopia_mount).await?;
	if let Some(parent) = kopia_mount.parent() {
		sys::make_traversable(parent).await?;
	}
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

	Ok((kopia_mount.join(rel), taken_at, mounts))
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

/// A held capture's top-level mount path.
fn held_toplevel_mount(id: &str) -> PathBuf {
	PathBuf::from(format!("{HELD_TOPLEVEL_PREFIX}.{id}"))
}

/// Name for a snapshot subvolume promoted to a hold.
fn held_snapshot_name(id: &str) -> String {
	format!("{HELD_INFIX}{id}")
}

/// Promote this run's capture to a held one.
///
/// The snapshot subvolume is renamed out of the reaper's namespace and remounted
/// at the hold's own path, and the run's own mounts are released — so the next
/// run of this type finds its stable paths free and its reaper finds nothing of
/// ours to delete. Returns the path the held capture is readable at, and what it
/// takes to release it.
pub async fn hold(mounts: Mounts, id: &str, source: &Path) -> Result<(PathBuf, HeldCapture)> {
	let rel = source
		.strip_prefix(&mounts.kopia_mount)
		.map_err(|_| {
			miette!(
				"{} is not under the capture's mount {}",
				source.display(),
				mounts.kopia_mount.display()
			)
		})?
		.to_path_buf();
	let run_name = mounts
		.snapshot_path
		.file_name()
		.ok_or_else(|| miette!("the capture has no snapshot subvolume to hold"))?
		.to_owned();

	let held_toplevel = held_toplevel_mount(id);
	let held_name = held_snapshot_name(id);
	let held_snapshot = held_toplevel.join(&held_name);
	let held_mount = super::super::hold::hold_source_dir(id);

	// Mount the filesystem's top level at the hold's own path first: the rename
	// and the remount both have to outlive the run's mounts going away.
	sys::mkdir(&held_toplevel).await?;
	sys::run_ok(
		"mount",
		&[
			"-o",
			"subvolid=5",
			&mounts.fsdev,
			sys::path(&held_toplevel),
		],
	)
	.await?;

	// Same filesystem, so the subvolume keeps its contents and simply stops
	// matching the glob the reaper deletes by.
	let from = held_toplevel.join(&run_name);
	if let Err(err) = tokio::fs::rename(&from, &held_snapshot).await {
		sys::umount(&held_toplevel).await;
		sys::rmdir(&held_toplevel).await;
		return Err(err)
			.into_diagnostic()
			.wrap_err_with(|| format!("renaming {} to {}", from.display(), held_snapshot.display()));
	}

	info!(hold = %id, snapshot = %held_snapshot.display(), "held btrfs snapshot");

	// The run's stable per-type mount and its top-level mount are the next run's
	// to reuse and to reap, so hand them back now that the capture has moved.
	sys::umount(&mounts.kopia_mount).await;
	sys::rmdir(&mounts.kopia_mount).await;
	sys::umount(&mounts.toplevel_mount).await;
	sys::rmdir(&mounts.toplevel_mount).await;

	let capture = HeldCapture::Btrfs {
		toplevel_mount: held_toplevel,
		snapshot_path: held_snapshot,
		mount: held_mount.clone(),
	};

	sys::mkdir(&held_mount).await?;
	if let Some(parent) = held_mount.parent() {
		sys::make_traversable(parent).await?;
	}
	if let Err(err) = sys::run_ok(
		"mount",
		&[
			&mounts.fsdev,
			sys::path(&held_mount),
			"-o",
			&format!("subvol={held_name},X-mount.idmap={}", mounts.idmap),
		],
	)
	.await
	{
		// The capture itself survived the rename, so release what we can name and
		// report the failure rather than stranding a subvolume nothing records.
		let _ = release(&capture).await;
		return Err(err).wrap_err("mounting the held snapshot");
	}

	Ok((held_mount.join(rel), capture))
}

/// Release a capture that was promoted to a hold: the same teardown, rebuilt from
/// the hold's record rather than from the run that took it.
pub async fn release_held(toplevel_mount: &Path, snapshot_path: &Path, mount: &Path) -> Result<()> {
	teardown(Mounts {
		toplevel_mount: toplevel_mount.to_path_buf(),
		snapshot_path: snapshot_path.to_path_buf(),
		kopia_mount: mount.to_path_buf(),
		// Releasing only unmounts and deletes; nothing is mounted again, so the
		// details a remount would need aren't carried in the hold's record.
		fsdev: String::new(),
		idmap: String::new(),
	})
	.await
}

/// Sweep leftovers from a previously crashed run (hard reboot skips teardown):
/// the stable kopia mount, stray top-level mounts, and orphaned `bestool-kopia-*`
/// snapshot subvolumes. All best-effort.
async fn reap_stale(fsdev: &str, kopia_mount: &Path) {
	sys::umount(kopia_mount).await;

	if let Ok(entries) = sys::glob_prefix(TOPLEVEL_MOUNT_DIR, "bestool-btrfs-toplevel.") {
		for stale in entries {
			sys::umount(&stale).await;
			sys::rmdir(&stale).await;
		}
	}

	let reap_mount = PathBuf::from(format!("{TOPLEVEL_MOUNT_PREFIX}.reap-{}", sys::run_token()));
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
		let name = snapshot_name("deadbeef");
		assert_eq!(name, "bestool-kopia-deadbeef");
		assert!(name.starts_with(SNAPSHOT_INFIX));
	}

	/// The reaper deletes every subvolume carrying [`SNAPSHOT_INFIX`] and unmounts
	/// every top-level mount under its prefix, both by glob and both at the start
	/// of every run of any type. A held capture that matched either would be
	/// destroyed by the next backup, so neither name may match.
	#[test]
	fn held_names_are_outside_the_reapers_globs() {
		let name = held_snapshot_name("tamanu-postgres-20260814T054412Z");
		assert!(
			!name.starts_with(SNAPSHOT_INFIX),
			"{name} would be deleted by the reaper"
		);

		let mount = held_toplevel_mount("tamanu-postgres-20260814T054412Z");
		let swept = format!("{TOPLEVEL_MOUNT_PREFIX}.");
		assert!(
			!mount.to_string_lossy().starts_with(&swept),
			"{} would be unmounted by the reaper",
			mount.display()
		);
		// …and still under the state directory the reaper globs within, which is
		// what makes the near-miss worth asserting.
		assert!(mount.starts_with(TOPLEVEL_MOUNT_DIR));
	}

	#[test]
	fn held_mounts_are_keyed_by_hold_not_by_backup_type() {
		let one = held_toplevel_mount("tamanu-postgres-20260814T054412Z");
		let two = held_toplevel_mount("tamanu-postgres-20260814T060000Z");
		assert_ne!(one, two, "two holds of one type must not collide");
	}

	#[test]
	fn stable_mount_is_per_type_and_fixed() {
		// The mount path is exactly the shared per-type source dir, so a host can
		// migrate between backends and keep one kopia history.
		assert_eq!(
			stable_kopia_mount("tamanu-postgres"),
			super::super::stable_source_dir("tamanu-postgres")
		);
	}

	#[test]
	fn toplevel_mount_path() {
		assert_eq!(
			toplevel_mount("cafef00d"),
			PathBuf::from("/var/lib/bestool/bestool-btrfs-toplevel.cafef00d")
		);
	}

	#[test]
	fn reaper_glob_matches_toplevel_and_reap_mounts() {
		// Both a run's top-level mount and the reaper's own scratch mount must be
		// swept by the glob, so a crashed reaper leaves nothing behind.
		let file = |p: &PathBuf| p.file_name().unwrap().to_string_lossy().into_owned();
		assert!(file(&toplevel_mount("abc")).starts_with("bestool-btrfs-toplevel."));
		let reap = PathBuf::from(format!("{TOPLEVEL_MOUNT_PREFIX}.reap-abc"));
		assert!(file(&reap).starts_with("bestool-btrfs-toplevel."));
	}
}
