//! Crash-consistent btrfs snapshot of a postgres cluster.
//!
//! Mirrors the proven `kopia-backup-postgres-btrfs.sh` approach: take an atomic,
//! read-only btrfs snapshot of the subvolume the data directory lives on (which
//! includes `pg_wal`), mount it read-only at a **stable** path (so kopia's
//! snapshot history/dedup attribute to one source), and hand kopia the cluster
//! directory within. No `pg_backup_start`/`backup_label` — the snapshot restores
//! by plain crash recovery.
//!
//! The privileged steps (mount, `btrfs subvolume snapshot`, id lookups) can't run
//! in unit tests; the pure helpers (names, idmap, paths) are tested and the whole
//! flow is verified on a real btrfs host per the plan.

use std::{
	path::{Path, PathBuf},
	process::Stdio,
};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use tracing::{info, warn};

use super::resolve::ResolvedCluster;

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

/// The stable mount path for a backup type (see
/// [`super::stable_source_dir`]).
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

/// The btrfs `X-mount.idmap` mapping postgres's uid/gid to kopia's, so the kopia
/// user can read the postgres-owned files in the read-only snapshot.
fn idmap(postgres_uid: u32, kopia_uid: u32, postgres_gid: u32, kopia_gid: u32) -> String {
	format!("u:{postgres_uid}:{kopia_uid}:1 g:{postgres_gid}:{kopia_gid}:1")
}

/// The cluster directory's path relative to the filesystem mountpoint.
fn relative_data_path(data_dir: &Path, base_mount: &Path) -> Result<PathBuf> {
	data_dir
		.strip_prefix(base_mount)
		.map(Path::to_path_buf)
		.map_err(|_| {
			miette::miette!(
				"data dir {} is not under its mountpoint {}",
				data_dir.display(),
				base_mount.display()
			)
		})
}

/// Take the snapshot and mount it; returns the kopia source path and the
/// teardown state. Caller must always pass the result to [`teardown`].
pub async fn prepare(resolved: &ResolvedCluster, backup_type: &str) -> Result<(PathBuf, Mounts)> {
	let pid = std::process::id();
	let base_mount = findmnt_target(&resolved.data_dir).await?;
	let rel = relative_data_path(&resolved.data_dir, &base_mount)?;
	let fsdev = format!(
		"/dev/disk/by-uuid/{}",
		findmnt_field("UUID", &resolved.data_dir).await?
	);

	let map = idmap(
		uid_of("postgres").await?,
		uid_of("kopia").await?,
		gid_of("postgres").await?,
		gid_of("kopia").await?,
	);

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

	mkdir(&toplevel_mount).await?;
	run_ok("mount", &["-o", "subvolid=5", &fsdev, path(&toplevel_mount)]).await?;

	info!(snapshot = %snapshot_path.display(), "creating read-only btrfs snapshot");
	run_ok(
		"btrfs",
		&[
			"subvolume",
			"snapshot",
			"-r",
			path(&base_mount),
			path(&snapshot_path),
		],
	)
	.await?;
	mounts.snapshot_path = snapshot_path.clone();

	mkdir(&kopia_mount).await?;
	run_ok(
		"mount",
		&[
			&fsdev,
			path(&kopia_mount),
			"-o",
			&format!("subvol={snapshot_name},X-mount.idmap={map}"),
		],
	)
	.await?;
	mounts.kopia_mount = kopia_mount.clone();

	let source = kopia_mount.join(rel);
	Ok((source, mounts))
}

/// Release a prepared snapshot: unmount the kopia mount, delete the snapshot
/// subvolume, unmount and remove the top-level mount. Best-effort throughout.
pub async fn teardown(mounts: Mounts) -> Result<()> {
	if !mounts.kopia_mount.as_os_str().is_empty() {
		umount(&mounts.kopia_mount).await;
		rmdir(&mounts.kopia_mount).await;
	}
	if !mounts.snapshot_path.as_os_str().is_empty() {
		let _ = run_ok(
			"btrfs",
			&["subvolume", "delete", path(&mounts.snapshot_path)],
		)
		.await
		.inspect_err(|err| warn!("deleting snapshot subvolume failed: {err}"));
	}
	if !mounts.toplevel_mount.as_os_str().is_empty() {
		umount(&mounts.toplevel_mount).await;
		rmdir(&mounts.toplevel_mount).await;
	}
	Ok(())
}

/// Sweep leftovers from a previously crashed run (hard reboot skips teardown):
/// the stable kopia mount, stray top-level mounts, and orphaned `bestool-kopia-*`
/// snapshot subvolumes. All best-effort.
async fn reap_stale(fsdev: &str, kopia_mount: &Path) {
	umount(kopia_mount).await;

	if let Ok(entries) = glob_prefix("/mnt", "bestool-btrfs-toplevel.") {
		for stale in entries {
			umount(&stale).await;
			rmdir(&stale).await;
		}
	}

	let reap_mount = PathBuf::from(format!("{TOPLEVEL_MOUNT_PREFIX}-reap.{}", std::process::id()));
	if mkdir(&reap_mount).await.is_ok()
		&& run_ok("mount", &["-o", "subvolid=5", fsdev, path(&reap_mount)])
			.await
			.is_ok()
	{
		if let Ok(subs) = glob_prefix(path(&reap_mount), SNAPSHOT_INFIX) {
			for sub in subs {
				let _ = run_ok("btrfs", &["subvolume", "delete", path(&sub)]).await;
			}
		}
		umount(&reap_mount).await;
	}
	rmdir(&reap_mount).await;
}

// --- thin command wrappers -------------------------------------------------

fn path(p: &Path) -> &str {
	p.to_str().unwrap_or_default()
}

async fn run_ok(program: &str, args: &[&str]) -> Result<()> {
	let output = tokio::process::Command::new(program)
		.args(args)
		.stdin(Stdio::null())
		.output()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("spawning {program}"))?;
	if !output.status.success() {
		bail!(
			"{program} {} failed: {}",
			args.join(" "),
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	Ok(())
}

async fn capture(program: &str, args: &[&str]) -> Result<String> {
	let output = tokio::process::Command::new(program)
		.args(args)
		.stdin(Stdio::null())
		.output()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("spawning {program}"))?;
	if !output.status.success() {
		bail!(
			"{program} {} failed: {}",
			args.join(" "),
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

async fn findmnt_target(data_dir: &Path) -> Result<PathBuf> {
	Ok(PathBuf::from(findmnt_field("TARGET", data_dir).await?))
}

async fn findmnt_field(field: &str, data_dir: &Path) -> Result<String> {
	capture("findmnt", &["-no", field, "--target", path(data_dir)]).await
}

async fn uid_of(user: &str) -> Result<u32> {
	parse_id(&capture("id", &["-u", user]).await?, user)
}

async fn gid_of(user: &str) -> Result<u32> {
	parse_id(&capture("id", &["-g", user]).await?, user)
}

fn parse_id(out: &str, user: &str) -> Result<u32> {
	out.trim()
		.parse()
		.into_diagnostic()
		.wrap_err_with(|| format!("parsing id for {user}: {out:?}"))
}

async fn mkdir(dir: &Path) -> Result<()> {
	tokio::fs::create_dir_all(dir)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating {}", dir.display()))
}

async fn umount(dir: &Path) {
	if is_mountpoint(dir).await {
		let _ = run_ok("umount", &[path(dir)]).await;
	}
}

async fn rmdir(dir: &Path) {
	let _ = tokio::fs::remove_dir(dir).await;
}

async fn is_mountpoint(dir: &Path) -> bool {
	tokio::process::Command::new("mountpoint")
		.arg("-q")
		.arg(dir)
		.stdin(Stdio::null())
		.status()
		.await
		.map(|s| s.success())
		.unwrap_or(false)
}

/// Directory entries whose file name starts with `prefix`.
fn glob_prefix(dir: impl AsRef<Path>, prefix: &str) -> Result<Vec<PathBuf>> {
	let mut out = Vec::new();
	for entry in std::fs::read_dir(dir.as_ref()).into_diagnostic()? {
		let entry = entry.into_diagnostic()?;
		if entry
			.file_name()
			.to_string_lossy()
			.starts_with(prefix)
		{
			out.push(entry.path());
		}
	}
	Ok(out)
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
	fn idmap_format() {
		assert_eq!(idmap(114, 997, 120, 995), "u:114:997:1 g:120:995:1");
	}

	#[test]
	fn relative_data_path_strips_mountpoint() {
		let rel = relative_data_path(
			Path::new("/var/lib/postgresql/16/main"),
			Path::new("/var/lib/postgresql"),
		)
		.unwrap();
		assert_eq!(rel, PathBuf::from("16/main"));
	}

	#[test]
	fn relative_data_path_rejects_outside() {
		assert!(relative_data_path(Path::new("/srv/pg"), Path::new("/var/lib/postgresql")).is_err());
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
