//! Held captures: a run's capture retained on the device after the run.
//!
//! A backup method prepares its capture for the *run* — at a mount path keyed by
//! backup type, and (btrfs, thin-LVM) under a name whose infix exists so the next
//! run's reaper can glob orphans. Retaining a capture is therefore not a skipped
//! teardown: the capture has to be promoted out of that run-owned namespace first,
//! or the next run of the same type unmounts it or deletes it outright. Each
//! backend's promotion lives in its own module; this one owns the record that
//! outlives the process and the release that undoes the promotion.
//!
//! The record is written to disk because a hold outlives the daemon and the
//! machine: it carries enough to find the capture, describe it, and release it
//! without the run that made it. Its shape is deliberately independent of the
//! backends' internal teardown structs — a hold taken before a bestool upgrade
//! still has to be droppable after one.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// A capture retained on the device, as stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldRecord {
	/// The hold's id, and the stem of its record file.
	pub id: String,
	/// The backup type the capture came from.
	pub backup_type: String,
	/// The instant the data froze, where the capture has one. A streamed base
	/// backup represents an interval rather than a point and records none.
	#[serde(default)]
	pub taken_at: Option<Timestamp>,
	/// When the capture was retained.
	pub held_at: Timestamp,
	/// Where the capture is readable — the path a restore reads from.
	pub source: PathBuf,
	/// Whether the run that took this capture also uploaded it.
	pub uploaded: bool,
	/// What to release when the hold is dropped.
	pub capture: HeldCapture,
}

/// The retained capture itself, in the terms its backend needs to release it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "kebab-case")]
pub enum HeldCapture {
	Btrfs {
		toplevel_mount: PathBuf,
		snapshot_path: PathBuf,
		mount: PathBuf,
	},
	Lvm {
		vg: String,
		lv: String,
		mount: PathBuf,
	},
	/// Carried on every platform even though only Windows can release it: a
	/// record is read by whatever bestool runs next on the host that wrote it,
	/// and failing to parse it would strand the hold rather than report it.
	Vss {
		shadow_id: String,
		junction: PathBuf,
	},
	BaseBackup {
		root: PathBuf,
	},
}

impl HeldCapture {
	/// The backend's name, for diagnostics and listings.
	pub fn backend(&self) -> &'static str {
		match self {
			Self::Btrfs { .. } => "btrfs",
			Self::Lvm { .. } => "lvm",
			Self::Vss { .. } => "vss",
			Self::BaseBackup { .. } => "basebackup",
		}
	}
}

/// Where hold records live. Under the daemon's own state directory on Unix, and
/// the machine-wide application data directory on Windows: a hold survives the
/// daemon restarting and the machine rebooting, so it can't live anywhere
/// per-boot or per-user.
pub fn records_dir() -> PathBuf {
	#[cfg(unix)]
	{
		PathBuf::from("/var/lib/bestool/held-snapshots")
	}
	#[cfg(not(unix))]
	{
		std::env::var_os("ProgramData")
			.map(PathBuf::from)
			.unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
			.join("bestool")
			.join("held-snapshots")
	}
}

/// The record file for a hold id.
fn record_path(id: &str) -> PathBuf {
	records_dir().join(format!("{id}.json"))
}

/// Where a held capture is exposed, keyed by hold rather than by backup type so
/// a later run of the same type neither disturbs a hold nor is disturbed by one.
///
/// A held Windows shadow copy is exposed on its own volume instead, since a
/// junction can't cross volumes; it derives its path from the capture's volume.
pub fn hold_source_dir(id: &str) -> PathBuf {
	#[cfg(unix)]
	{
		PathBuf::from("/var/lib/bestool/held-source").join(id)
	}
	#[cfg(not(unix))]
	{
		std::env::var_os("ProgramData")
			.map(PathBuf::from)
			.unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
			.join("bestool")
			.join("held-source")
			.join(id)
	}
}

/// Mint a hold id from the backup type and the moment the capture represents.
/// Sortable, typeable at a prompt, and meaningful in a listing — an operator
/// picking a rollback point is choosing by time, so the time is in the name.
pub fn mint_id(backup_type: &str, at: Timestamp) -> String {
	let stamp = at.strftime("%Y%m%dT%H%M%SZ");
	format!("{backup_type}-{stamp}")
}

/// Write a hold record, creating the records directory if needed.
pub async fn save(record: &HoldRecord) -> Result<()> {
	let dir = records_dir();
	tokio::fs::create_dir_all(&dir)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating {}", dir.display()))?;

	let path = record_path(&record.id);
	let json = serde_json::to_vec_pretty(record)
		.into_diagnostic()
		.wrap_err("serialising the hold record")?;
	tokio::fs::write(&path, &json)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("writing {}", path.display()))?;
	debug!(id = %record.id, path = %path.display(), "wrote hold record");
	Ok(())
}

/// Read one hold record by id.
pub async fn load(id: &str) -> Result<HoldRecord> {
	let path = record_path(id);
	let json = tokio::fs::read(&path)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("no hold {id:?} on this device ({})", path.display()))?;
	parse(&json).wrap_err_with(|| format!("reading {}", path.display()))
}

/// Every hold record on the device, oldest capture first. A record that can't be
/// parsed is warned about and skipped rather than failing the listing: one bad
/// file must not hide the others from an operator looking for a rollback point.
pub async fn list() -> Result<Vec<HoldRecord>> {
	let dir = records_dir();
	let mut entries = match tokio::fs::read_dir(&dir).await {
		Ok(entries) => entries,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(err) => {
			return Err(err)
				.into_diagnostic()
				.wrap_err_with(|| format!("reading {}", dir.display()));
		}
	};

	let mut records = Vec::new();
	while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
		let path = entry.path();
		if path.extension().is_none_or(|ext| ext != "json") {
			continue;
		}
		match tokio::fs::read(&path).await.into_diagnostic().and_then(|j| parse(&j)) {
			Ok(record) => records.push(record),
			Err(err) => warn!("skipping unreadable hold record {}: {err}", path.display()),
		}
	}
	records.sort_by_key(|record| record.taken_at.unwrap_or(record.held_at));
	Ok(records)
}

fn parse(json: &[u8]) -> Result<HoldRecord> {
	serde_json::from_slice(json)
		.into_diagnostic()
		.wrap_err("parsing the hold record")
}

/// Remove a hold's record, leaving its capture alone.
pub async fn remove_record(id: &str) -> Result<()> {
	let path = record_path(id);
	match tokio::fs::remove_file(&path).await {
		Ok(()) => Ok(()),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
		Err(err) => Err(err)
			.into_diagnostic()
			.wrap_err_with(|| format!("removing {}", path.display())),
	}
}

/// Release a held capture: undo the promotion and free the underlying snapshot,
/// logical volume, shadow copy, or staged tree.
///
/// A capture that has already gone — evicted, swept by a hand-run tool, or lost
/// with its volume — is not an error. The hold is being dropped either way, and
/// the operator needs the record gone more than they need the failure.
pub async fn release(capture: &HeldCapture) -> Result<()> {
	match capture {
		HeldCapture::Btrfs {
			toplevel_mount,
			snapshot_path,
			mount,
		} => super::postgresql::btrfs::release_held(toplevel_mount, snapshot_path, mount).await,
		HeldCapture::Lvm { vg, lv, mount } => super::postgresql::lvm::release_held(vg, lv, mount).await,
		HeldCapture::Vss { shadow_id, junction } => release_vss(shadow_id, junction).await,
		HeldCapture::BaseBackup { root } => super::postgresql::basebackup::teardown(root.clone()).await,
	}
}

#[cfg(windows)]
async fn release_vss(shadow_id: &str, junction: &Path) -> Result<()> {
	super::postgresql::vss::release_held(shadow_id, junction).await
}

#[cfg(not(windows))]
async fn release_vss(shadow_id: &str, _junction: &Path) -> Result<()> {
	bail!("hold {shadow_id} holds a Windows shadow copy, which only Windows can release")
}

#[cfg(test)]
mod tests {
	use super::*;

	fn record(capture: HeldCapture) -> HoldRecord {
		HoldRecord {
			id: "tamanu-postgres-20260814T054412Z".into(),
			backup_type: "tamanu-postgres".into(),
			taken_at: Some("2026-08-14T05:44:12Z".parse().unwrap()),
			held_at: "2026-08-14T11:02:00Z".parse().unwrap(),
			source: PathBuf::from("/var/lib/bestool/held-source/x/16/main"),
			uploaded: true,
			capture,
		}
	}

	/// The on-disk shape has to survive a bestool upgrade: a hold taken before one
	/// is dropped after it, so every backend's record round-trips.
	#[test]
	fn every_backend_round_trips() {
		let captures = [
			HeldCapture::Btrfs {
				toplevel_mount: "/run/bestool-toplevel".into(),
				snapshot_path: "/run/bestool-toplevel/bestool-held-x".into(),
				mount: "/var/lib/bestool/held-source/x".into(),
			},
			HeldCapture::Lvm {
				vg: "vg0".into(),
				lv: "bestool-held-x".into(),
				mount: "/var/lib/bestool/held-source/x".into(),
			},
			HeldCapture::Vss {
				shadow_id: "{deadbeef-0000-0000-0000-000000000000}".into(),
				junction: r"C:\bestool-backup-shadow\held\x".into(),
			},
			HeldCapture::BaseBackup {
				root: "/var/lib/bestool/held-source/x".into(),
			},
		];

		for capture in captures {
			let backend = capture.backend();
			let original = record(capture);
			let json = serde_json::to_vec(&original).unwrap();
			let parsed = parse(&json).unwrap();
			assert_eq!(parsed.id, original.id);
			assert_eq!(parsed.backup_type, original.backup_type);
			assert_eq!(parsed.taken_at, original.taken_at);
			assert_eq!(parsed.source, original.source);
			assert!(parsed.uploaded);
			assert_eq!(parsed.capture.backend(), backend);
		}
	}

	/// A base-backup capture has no freeze instant, and the record says so rather
	/// than substituting the time it was held.
	#[test]
	fn a_capture_without_a_freeze_instant_records_none() {
		let mut original = record(HeldCapture::BaseBackup {
			root: "/var/lib/bestool/held-source/x".into(),
		});
		original.taken_at = None;
		let parsed = parse(&serde_json::to_vec(&original).unwrap()).unwrap();
		assert_eq!(parsed.taken_at, None);
		assert_eq!(parsed.held_at, original.held_at);
	}

	#[test]
	fn ids_are_sortable_and_carry_the_type_and_time() {
		let earlier = mint_id("tamanu-postgres", "2026-08-14T05:44:12Z".parse().unwrap());
		let later = mint_id("tamanu-postgres", "2026-08-14T06:00:00Z".parse().unwrap());
		assert_eq!(earlier, "tamanu-postgres-20260814T054412Z");
		assert!(earlier < later, "{earlier} should sort before {later}");
	}
}
