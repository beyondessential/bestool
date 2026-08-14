//! Crash-consistent thin-LVM snapshot of a postgres cluster.
//!
//! The thin-pool analogue of the btrfs path: take a thin snapshot of the LV the
//! data directory lives on (CoW within the pool, no read-copy), mount it
//! read-only at a stable path, and hand kopia the cluster directory within. No
//! `pg_backup_start`/`backup_label` — it restores by plain crash recovery.
//!
//! Only reached for **thin** LVs (a thick LV's snapshot is costly, so those go
//! to `pg_basebackup`). The privileged `lvcreate`/`mount` steps are verified
//! on-host; the pure helpers (parsing, mount options) are unit-tested.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use miette::{Context as _, Result, bail, miette};
use tracing::{info, warn};

use super::{
	super::hold::{HeldCapture, release},
	resolve::ResolvedCluster,
	sys,
};

/// Infix marking our ephemeral snapshot LVs, so the reaper never matches a live LV.
const SNAPSHOT_INFIX: &str = "bestool-kopia-";

/// Infix marking a snapshot LV promoted to a hold. Deliberately not
/// [`SNAPSHOT_INFIX`]: the reaper removes every LV carrying that one, so a held
/// capture left under it would be destroyed by the next run of any type.
const HELD_INFIX: &str = "bestool-held-";

/// Teardown state for a prepared thin-LVM snapshot, released by [`teardown`].
#[derive(Debug)]
pub struct Snapshot {
	/// The volume group the snapshot LV lives in.
	vg: String,
	/// The ephemeral snapshot LV's name.
	lv: String,
	/// The stable read-only mount kopia reads from.
	kopia_mount: PathBuf,
	/// The filesystem type, for mounting the capture again elsewhere.
	fstype: String,
	/// The postgres-to-kopia id map the kopia mount was made with.
	idmap: String,
}

fn snapshot_name(token: &str) -> String {
	format!("{SNAPSHOT_INFIX}{token}")
}

/// Parse `lvs --noheadings -o vg_name,lv_name <dev>` output into `(vg, lv)`.
fn parse_vg_lv(output: &str) -> Result<(String, String)> {
	let mut fields = output.split_whitespace();
	match (fields.next(), fields.next()) {
		(Some(vg), Some(lv)) => Ok((vg.to_owned(), lv.to_owned())),
		_ => bail!("could not parse vg/lv from lvs output: {output:?}"),
	}
}

/// Mount options for a read-only snapshot mount, idmapped for the kopia user.
/// XFS refuses a second mount with a duplicate fs UUID without `nouuid`.
fn mount_options(fstype: &str, idmap: &str) -> String {
	let mut opts = vec!["ro".to_owned()];
	if fstype == "xfs" {
		opts.push("nouuid".to_owned());
	}
	opts.push(format!("X-mount.idmap={idmap}"));
	opts.join(",")
}

/// Take a thin snapshot and mount it; returns the kopia source path and the
/// teardown state. Caller must always pass the result to [`teardown`].
pub async fn prepare(
	resolved: &ResolvedCluster,
	backup_type: &str,
) -> Result<(PathBuf, Timestamp, Snapshot)> {
	let token = sys::run_token();
	let base_mount = sys::findmnt_target(&resolved.data_dir).await?;
	let rel = sys::relative_data_path(&resolved.data_dir, &base_mount)?;
	let source = sys::findmnt_field("SOURCE", &resolved.data_dir).await?;
	let fstype = sys::findmnt_field("FSTYPE", &resolved.data_dir).await?;
	let (vg, lv) = parse_vg_lv(
		&sys::capture("lvs", &["--noheadings", "-o", "vg_name,lv_name", &source]).await?,
	)?;
	let map = sys::postgres_to_kopia_idmap().await?;

	let kopia_mount = super::stable_source_dir(backup_type);
	reap_stale(&vg, &kopia_mount).await;

	let snapshot_lv = snapshot_name(&token);
	let mut snapshot = Snapshot {
		vg: vg.clone(),
		lv: String::new(),
		kopia_mount: PathBuf::new(),
		fstype: fstype.clone(),
		idmap: map.clone(),
	};

	info!(vg = %vg, lv = %snapshot_lv, "creating thin-LVM snapshot");
	sys::run_ok(
		"lvcreate",
		&["--snapshot", "--name", &snapshot_lv, &format!("{vg}/{lv}")],
	)
	.await?;
	// The snapshot volume now exists: this is the instant the data froze.
	let taken_at = Timestamp::now();
	snapshot.lv = snapshot_lv.clone();

	// Thin snapshots carry the activation-skip flag; -K overrides it.
	sys::run_ok(
		"lvchange",
		&["-ay", "-K", &format!("{vg}/{snapshot_lv}")],
	)
	.await?;

	sys::mkdir(&kopia_mount).await?;
	if let Some(parent) = kopia_mount.parent() {
		sys::make_traversable(parent).await?;
	}
	sys::run_ok(
		"mount",
		&[
			&format!("/dev/{vg}/{snapshot_lv}"),
			sys::path(&kopia_mount),
			"-o",
			&mount_options(&fstype, &map),
		],
	)
	.await?;
	snapshot.kopia_mount = kopia_mount.clone();

	Ok((kopia_mount.join(rel), taken_at, snapshot))
}

/// Release a prepared snapshot: unmount, deactivate, and remove the snapshot LV.
pub async fn teardown(snapshot: Snapshot) -> Result<()> {
	if !snapshot.kopia_mount.as_os_str().is_empty() {
		sys::umount(&snapshot.kopia_mount).await;
		sys::rmdir(&snapshot.kopia_mount).await;
	}
	if !snapshot.lv.is_empty() {
		let target = format!("{}/{}", snapshot.vg, snapshot.lv);
		let _ = sys::run_ok("lvchange", &["-an", &target]).await;
		let _ = sys::run_ok("lvremove", &["-f", &target])
			.await
			.map_err(|err| miette!("removing snapshot LV {target}: {err}"))
			.inspect_err(|err| warn!("{err}"));
	}
	Ok(())
}

/// Name for a snapshot LV promoted to a hold. LVM accepts a restricted character
/// set for volume names, so anything outside it becomes a hyphen; the hold's
/// record carries the exact name, so the mapping only has to be valid, not
/// reversible.
fn held_snapshot_name(id: &str) -> String {
	let sanitised: String = id
		.chars()
		.map(|c| if c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '.' | '-') { c } else { '-' })
		.collect();
	format!("{HELD_INFIX}{sanitised}")
}

/// Promote this run's capture to a held one.
///
/// The snapshot LV is renamed out of the reaper's namespace and remounted at the
/// hold's own path, and the run's stable per-type mount is handed back. Returns
/// the path the held capture is readable at, and what it takes to release it.
pub async fn hold(snapshot: Snapshot, id: &str, source: &Path) -> Result<(PathBuf, HeldCapture)> {
	let rel = source
		.strip_prefix(&snapshot.kopia_mount)
		.map_err(|_| {
			miette!(
				"{} is not under the capture's mount {}",
				source.display(),
				snapshot.kopia_mount.display()
			)
		})?
		.to_path_buf();
	if snapshot.lv.is_empty() {
		bail!("the capture has no snapshot volume to hold");
	}

	let held_lv = held_snapshot_name(id);
	let held_mount = super::super::hold::hold_source_dir(id);

	// Free the per-type mount first: it belongs to the next run, and the capture
	// is about to be reachable at the hold's own path instead.
	sys::umount(&snapshot.kopia_mount).await;
	sys::rmdir(&snapshot.kopia_mount).await;

	sys::run_ok(
		"lvrename",
		&[&snapshot.vg, &snapshot.lv, &held_lv],
	)
	.await
	.wrap_err_with(|| format!("renaming {}/{} to {held_lv}", snapshot.vg, snapshot.lv))?;
	info!(hold = %id, vg = %snapshot.vg, lv = %held_lv, "held thin-LVM snapshot");

	let capture = HeldCapture::Lvm {
		vg: snapshot.vg.clone(),
		lv: held_lv.clone(),
		mount: held_mount.clone(),
	};

	sys::mkdir(&held_mount).await?;
	if let Some(parent) = held_mount.parent() {
		sys::make_traversable(parent).await?;
	}
	if let Err(err) = sys::run_ok(
		"mount",
		&[
			&format!("/dev/{}/{held_lv}", snapshot.vg),
			sys::path(&held_mount),
			"-o",
			&mount_options(&snapshot.fstype, &snapshot.idmap),
		],
	)
	.await
	{
		// The capture itself survived the rename, so release what we can name and
		// report the failure rather than stranding an LV nothing records.
		let _ = release(&capture).await;
		return Err(err).wrap_err("mounting the held snapshot");
	}

	Ok((held_mount.join(rel), capture))
}

/// Whether a held capture's snapshot volume still exists.
pub async fn held_present(vg: &str, lv: &str) -> bool {
	sys::run_ok("lvs", &[&format!("{vg}/{lv}")]).await.is_ok()
}

/// Release a capture that was promoted to a hold: the same teardown, rebuilt from
/// the hold's record rather than from the run that took it.
pub async fn release_held(vg: &str, lv: &str, mount: &Path) -> Result<()> {
	teardown(Snapshot {
		vg: vg.to_owned(),
		lv: lv.to_owned(),
		kopia_mount: mount.to_path_buf(),
		// Releasing only unmounts and removes; nothing is mounted again, so the
		// details a remount would need aren't carried in the hold's record.
		fstype: String::new(),
		idmap: String::new(),
	})
	.await
}

/// Sweep leftover `bestool-kopia-*` snapshot LVs from a crashed run.
async fn reap_stale(vg: &str, kopia_mount: &std::path::Path) {
	sys::umount(kopia_mount).await;
	let Ok(list) = sys::capture("lvs", &["--noheadings", "-o", "lv_name", vg]).await else {
		return;
	};
	for name in list.lines().map(str::trim).filter(|n| n.starts_with(SNAPSHOT_INFIX)) {
		let target = format!("{vg}/{name}");
		let _ = sys::run_ok("lvchange", &["-an", &target]).await;
		let _ = sys::run_ok("lvremove", &["-f", &target]).await;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The reaper removes every LV carrying [`SNAPSHOT_INFIX`], at the start of
	/// every run of any type, so a held capture that matched would be destroyed by
	/// the next backup.
	#[test]
	fn held_lv_names_are_outside_the_reapers_glob() {
		let name = held_snapshot_name("tamanu-postgres-20260814T054412Z");
		assert!(!name.starts_with(SNAPSHOT_INFIX), "{name} would be reaped");
		assert!(name.starts_with(HELD_INFIX));
	}

	/// LVM accepts a restricted character set for volume names, so a backup type
	/// carrying anything else still has to yield a usable name.
	#[test]
	fn held_lv_names_are_valid_lvm_names() {
		let name = held_snapshot_name("odd type/name:v2-20260814T054412Z");
		assert!(
			name.chars()
				.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '.' | '-')),
			"{name} is not a valid LVM volume name"
		);
	}

	#[test]
	fn parses_vg_lv_from_padded_output() {
		assert_eq!(
			parse_vg_lv("  ubuntu-vg ubuntu-lv ").unwrap(),
			("ubuntu-vg".to_owned(), "ubuntu-lv".to_owned())
		);
		assert!(parse_vg_lv("").is_err());
	}

	#[test]
	fn mount_options_add_nouuid_only_for_xfs() {
		assert_eq!(
			mount_options("ext4", "u:1:2:1 g:3:4:1"),
			"ro,X-mount.idmap=u:1:2:1 g:3:4:1"
		);
		assert_eq!(
			mount_options("xfs", "u:1:2:1 g:3:4:1"),
			"ro,nouuid,X-mount.idmap=u:1:2:1 g:3:4:1"
		);
	}

	#[test]
	fn snapshot_name_carries_reaper_infix() {
		assert!(snapshot_name("deadbeef").starts_with(SNAPSHOT_INFIX));
	}
}
