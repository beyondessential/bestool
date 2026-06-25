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

use std::path::PathBuf;

use miette::{Result, bail, miette};
use tracing::{info, warn};

use super::{resolve::ResolvedCluster, sys};

/// Infix marking our ephemeral snapshot LVs, so the reaper never matches a live LV.
const SNAPSHOT_INFIX: &str = "bestool-kopia-";

/// Teardown state for a prepared thin-LVM snapshot, released by [`teardown`].
#[derive(Debug)]
pub struct Snapshot {
	/// The volume group the snapshot LV lives in.
	vg: String,
	/// The ephemeral snapshot LV's name.
	lv: String,
	/// The stable read-only mount kopia reads from.
	kopia_mount: PathBuf,
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
pub async fn prepare(resolved: &ResolvedCluster, backup_type: &str) -> Result<(PathBuf, Snapshot)> {
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
	};

	info!(vg = %vg, lv = %snapshot_lv, "creating thin-LVM snapshot");
	sys::run_ok(
		"lvcreate",
		&["--snapshot", "--name", &snapshot_lv, &format!("{vg}/{lv}")],
	)
	.await?;
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

	Ok((kopia_mount.join(rel), snapshot))
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
