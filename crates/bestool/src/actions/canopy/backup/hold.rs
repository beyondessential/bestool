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
	/// What the backend noted at the freeze, so a later in-place restore can ask
	/// the filesystem what has diverged since instead of scanning for it.
	///
	/// Absent where the backend has nothing to offer, and on records written
	/// before it was kept. Never required: a restore without one compares the
	/// two trees itself.
	#[serde(default)]
	pub diverged_since: Option<DivergenceMark>,
}

/// A point in a filesystem's own change history, recorded when a capture froze.
///
/// Reading the divergence from the filesystem's metadata costs nothing like the
/// full read of both trees that finding it by hand does, so it is worth
/// recording even though nothing depends on it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DivergenceMark {
	/// The btrfs transaction generation the subvolume stood at when it was
	/// snapshotted. Everything written to it since carries a higher one.
	///
	/// A generation only means anything against the subvolume it was counted on:
	/// another subvolume, or a filesystem since recreated, has its own sequence
	/// that started again from low numbers, and asking it about this generation
	/// answers cleanly and wrongly. So the subvolume's UUID is recorded with it,
	/// and a restore that cannot match it declines — the same guard the change
	/// journal gets from its journal id. Absent on records written before it was
	/// kept, which therefore cannot be trusted as a basis.
	BtrfsGeneration {
		generation: u64,
		#[serde(default)]
		subvolume: Option<String>,
	},
	/// The NTFS change journal's identity and position when the shadow was
	/// taken. The journal is a fixed-size ring, so the id detects it having been
	/// recreated and the position detects it having wrapped past this point —
	/// either of which makes the record no longer an answer.
	UsnJournal {
		volume: PathBuf,
		journal_id: u64,
		usn: i64,
	},
}

/// The subvolume UUID out of `btrfs subvolume show`.
///
/// Lives with [`DivergenceMark`] because the capture side records it and the
/// restore side checks it, and the two must read the field the same way forever:
/// a divergence between two copies of this would turn the identity guard into a
/// permanent refusal, or into a wrong match.
///
/// Matched on the whole `UUID:` label, since `Parent UUID` and `Received UUID`
/// also end in it. A snapshot with no UUID of its own reports `-`.
pub fn parse_subvolume_uuid(output: &str) -> Option<String> {
	output
		.lines()
		.filter_map(|line| line.trim().strip_prefix("UUID:"))
		.map(str::trim)
		.find(|uuid| !uuid.is_empty() && *uuid != "-")
		.map(str::to_owned)
}

/// The retained capture itself, in the terms its backend needs to release it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "kebab-case")]
pub enum HeldCapture {
	Btrfs {
		toplevel_mount: PathBuf,
		snapshot_path: PathBuf,
		mount: PathBuf,
		/// The device the subvolume lives on. Releasing reaches it through the
		/// top-level mount, which may no longer be there, so remounting to delete
		/// it needs the device. `None` on records written before it was kept.
		#[serde(default)]
		fsdev: Option<String>,
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
	/// Whether the capture is reached through something separate from itself, and
	/// so can both lose that exposure and have it put back. [`capture_state`] and
	/// [`reattach`] both key off this: reporting a hold detached and then refusing
	/// to reattach it would send an operator to a command that cannot work.
	pub fn exposes_separately(&self) -> bool {
		match self {
			Self::Btrfs { .. } | Self::Lvm { .. } | Self::Vss { .. } => true,
			// The capture is the directory itself, so there is nothing to put back.
			Self::BaseBackup { .. } => false,
		}
	}

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

/// What state the capture a hold names is in.
///
/// A hold whose capture has gone is the failure worth catching: the operator
/// believes a rollback point exists when it does not, and nothing about the
/// record itself gives that away. A detached one is the recoverable case and
/// must be told apart, since releasing it as though it were gone would forget
/// the record and strand the capture it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureState {
	/// Readable where the record says it is.
	Present,
	/// The capture is there, but what exposes it is not mounted.
	Detached,
	/// Nothing of the capture is left.
	Gone,
}

/// Probed at the path a restore reads, not at whatever exposes it.
///
/// For a shadow copy the two are not interchangeable. Its junction substitutes a
/// `\??\GLOBALROOT\Device\HarddiskVolumeShadowCopyN` device path, and enumerating
/// the junction itself opens that device rather than the root directory of the
/// filesystem on it, so it comes back empty however healthy the copy is. Paths
/// *through* the junction resolve normally, which is why the capture reads fine
/// and only the probe of its root did not.
pub async fn capture_state(record: &HoldRecord) -> CaptureState {
	match &record.capture {
		HeldCapture::Btrfs {
			toplevel_mount,
			snapshot_path,
			mount,
			..
		} => {
			if !snapshot_path.exists() {
				if super::postgresql::btrfs::attached(toplevel_mount).await {
					// The top level is mounted and the subvolume still isn't there.
					CaptureState::Gone
				} else {
					CaptureState::Detached
				}
			} else if super::postgresql::btrfs::attached(mount).await {
				CaptureState::Present
			} else {
				// The subvolume is there but nothing exposes it, and a restore reads
				// the mount rather than the subvolume: reporting this present would
				// lay an empty directory over the cluster.
				CaptureState::Detached
			}
		}
		HeldCapture::Lvm { vg, lv, mount } => {
			if !super::postgresql::lvm::held_present(vg, lv).await {
				CaptureState::Gone
			} else if super::postgresql::lvm::attached(mount).await {
				CaptureState::Present
			} else {
				CaptureState::Detached
			}
		}
		HeldCapture::Vss { shadow_id, junction } => {
			if !vss_present(shadow_id, junction).await {
				CaptureState::Gone
			} else if tokio::fs::read_dir(&record.source).await.is_ok() {
				CaptureState::Present
			} else {
				// The shadow is there and the junction is not resolving. VSS can
				// renumber the device a junction points at, so the copy behind it
				// outlives what names it.
				CaptureState::Detached
			}
		}
		HeldCapture::BaseBackup { root } => {
			if root.exists() {
				CaptureState::Present
			} else {
				CaptureState::Gone
			}
		}
	}
}


#[cfg(windows)]
async fn vss_present(shadow_id: &str, _junction: &Path) -> bool {
	super::postgresql::vss::held_present(shadow_id).await
}

/// Off Windows the shadow itself can't be queried, so the mount is the only
/// evidence available — enough to list a record without claiming more.
#[cfg(not(windows))]
async fn vss_present(_shadow_id: &str, junction: &Path) -> bool {
	junction.exists()
}

/// Expose a detached capture again at the path its record names.
///
/// A hold's mount is made by whichever process took it, and the daemon that
/// takes one runs in its own mount namespace, so the mount is not visible to a
/// restore run from a shell and does not outlive the daemon. Reattaching from
/// the shell puts it where both can see it.
pub async fn reattach(capture: &HeldCapture) -> Result<()> {
	if !capture.exposes_separately() {
		bail!(
			"reattaching a {} capture is not supported; nothing exposes it separately",
			capture.backend()
		);
	}
	match capture {
		HeldCapture::Btrfs {
			toplevel_mount,
			snapshot_path,
			mount,
			fsdev,
		} => {
			super::postgresql::btrfs::reattach_held(
				toplevel_mount,
				snapshot_path,
				mount,
				fsdev.as_deref(),
			)
			.await
		}
		HeldCapture::Lvm { vg, lv, mount } => {
			super::postgresql::lvm::reattach_held(vg, lv, mount).await
		}
		HeldCapture::Vss { shadow_id, junction } => reattach_vss(shadow_id, junction).await,
		other => bail!(
			"reattaching a {} capture is not supported; nothing exposes it separately",
			other.backend()
		),
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
			fsdev,
			..
		} => {
			super::postgresql::btrfs::release_held(toplevel_mount, snapshot_path, mount, fsdev.as_deref())
				.await
		}
		HeldCapture::Lvm { vg, lv, mount } => super::postgresql::lvm::release_held(vg, lv, mount).await,
		HeldCapture::Vss { shadow_id, junction } => release_vss(shadow_id, junction).await,
		HeldCapture::BaseBackup { root } => super::postgresql::basebackup::teardown(root.clone()).await,
	}
}

#[cfg(windows)]
async fn reattach_vss(shadow_id: &str, junction: &Path) -> Result<()> {
	super::postgresql::vss::reattach_held(shadow_id, junction).await
}

/// Only the host that made the shadow can rebuild the junction to it.
#[cfg(not(windows))]
async fn reattach_vss(_shadow_id: &str, _junction: &Path) -> Result<()> {
	bail!("a shadow copy can only be reattached on the Windows host that holds it")
}

#[cfg(windows)]
async fn release_vss(shadow_id: &str, junction: &Path) -> Result<()> {
	super::postgresql::vss::release_held(shadow_id, junction).await
}

#[cfg(not(windows))]
async fn release_vss(shadow_id: &str, _junction: &Path) -> Result<()> {
	miette::bail!("hold {shadow_id} holds a Windows shadow copy, which only Windows can release")
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
			diverged_since: None,
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
				fsdev: Some("/dev/disk/by-uuid/deadbeef".into()),
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

	/// A hold taken before the divergence mark existed is still a rollback point,
	/// so its record has to keep parsing. Failing to would strand the hold rather
	/// than report it.
	#[test]
	fn a_record_written_before_the_divergence_mark_still_parses() {
		let json = br#"{
			"id": "x-20260814T054412Z",
			"backup_type": "x",
			"taken_at": "2026-08-14T05:44:12Z",
			"held_at": "2026-08-14T11:02:00Z",
			"source": "/var/lib/bestool/held-source/x",
			"uploaded": true,
			"capture": { "backend": "base-backup", "root": "/var/lib/bestool/held-source/x" }
		}"#;
		let parsed = parse(json).unwrap();
		assert_eq!(parsed.diverged_since, None);
	}

	const SHOW: &str = "\
pgsub
\tName: \t\t\tpgsub
\tUUID: \t\t\t9960cf5a-4a6d-a641-b986-3a71d4549d03
\tParent UUID: \t\t-
\tReceived UUID: \t\t-
\tGeneration: \t\t10
";

	#[test]
	fn reads_the_subvolumes_own_uuid_not_its_parents() {
		assert_eq!(
			parse_subvolume_uuid(SHOW).as_deref(),
			Some("9960cf5a-4a6d-a641-b986-3a71d4549d03")
		);
	}

	#[test]
	fn a_subvolume_with_no_uuid_at_all_is_not_identifiable() {
		assert_eq!(parse_subvolume_uuid("ERROR: not a subvolume"), None);
		assert_eq!(parse_subvolume_uuid("\tUUID: \t-\n"), None);
	}

	/// A btrfs mark from before the subvolume was recorded still parses; it is
	/// the restore that refuses to act on one, not the reader.
	#[test]
	fn a_btrfs_mark_without_its_subvolume_still_parses() {
		let json = br#"{"kind":"btrfs-generation","generation":4211}"#;
		let mark: DivergenceMark = serde_json::from_slice(json).unwrap();
		assert_eq!(
			mark,
			DivergenceMark::BtrfsGeneration {
				generation: 4_211,
				subvolume: None
			}
		);
	}

	/// The mark is what lets a later in-place restore ask the filesystem what
	/// diverged instead of reading both trees, so it has to survive the upgrade
	/// that sits between taking a hold and restoring from it.
	#[test]
	fn every_divergence_mark_round_trips() {
		let marks = [
			DivergenceMark::BtrfsGeneration {
				generation: 4_211,
				subvolume: Some("9960cf5a-4a6d-a641-b986-3a71d4549d03".into()),
			},
			DivergenceMark::UsnJournal {
				volume: PathBuf::from("C:"),
				journal_id: 0x01d5_f4e2_c3b1_a098,
				usn: 0x0012_3456,
			},
		];
		for mark in marks {
			let mut original = record(HeldCapture::BaseBackup {
				root: "/var/lib/bestool/held-source/x".into(),
			});
			original.diverged_since = Some(mark.clone());
			let parsed = parse(&serde_json::to_vec(&original).unwrap()).unwrap();
			assert_eq!(parsed.diverged_since, Some(mark));
		}
	}

	/// Releasing a btrfs hold reaches the subvolume through the top-level mount,
	/// so the device that mount needs has to survive the round trip. Without it
	/// the subvolume cannot be deleted and its space is never returned.
	#[test]
	fn a_btrfs_capture_keeps_what_it_takes_to_reach_the_subvolume() {
		let original = record(HeldCapture::Btrfs {
			toplevel_mount: "/run/bestool-toplevel".into(),
			snapshot_path: "/run/bestool-toplevel/bestool-held-x".into(),
			mount: "/var/lib/bestool/held-source/x".into(),
			fsdev: Some("/dev/disk/by-uuid/deadbeef".into()),
		});
		let parsed = parse(&serde_json::to_vec(&original).unwrap()).unwrap();
		let HeldCapture::Btrfs { fsdev, .. } = parsed.capture else {
			panic!("expected a btrfs capture");
		};
		assert_eq!(fsdev.as_deref(), Some("/dev/disk/by-uuid/deadbeef"));
	}

	/// Reattaching needs the device, and a record written before it was kept
	/// cannot be mounted again. The refusal has to say so rather than report
	/// success over a capture nothing exposed.
	#[tokio::test]
	async fn reattaching_without_a_device_refuses() {
		let scratch = std::env::temp_dir().join("bestool-hold-unit");
		let err = reattach(&HeldCapture::Btrfs {
			toplevel_mount: scratch.join("toplevel"),
			snapshot_path: scratch.join("toplevel/bestool-held-x"),
			mount: scratch.join("held-source/x"),
			fsdev: None,
		})
		.await
		.expect_err("a hold with no device cannot be reattached");
		assert!(err.to_string().contains("no device recorded"), "{err}");
	}

	/// Every backend that can report detached has to have a remedy: telling an
	/// operator to reattach something that cannot be reattached is worse than
	/// refusing up front. Asserted on the decision rather than by calling
	/// `reattach`, which would mount real storage.
	#[test]
	fn every_backend_that_can_detach_can_be_reattached() {
		for capture in [
			HeldCapture::Vss {
				shadow_id: "{deadbeef-0000-0000-0000-000000000000}".into(),
				junction: r"C:\bestool-backup-shadow\held\x".into(),
			},
			HeldCapture::Btrfs {
				toplevel_mount: "/nonexistent/toplevel".into(),
				snapshot_path: "/nonexistent/toplevel/bestool-held-x".into(),
				mount: "/nonexistent/held-source/x".into(),
				fsdev: Some("/dev/disk/by-uuid/deadbeef".into()),
			},
			HeldCapture::Lvm {
				vg: "vg0".into(),
				lv: "bestool-held-x".into(),
				mount: "/nonexistent/held-source/x".into(),
			},
		] {
			assert!(
				capture.exposes_separately(),
				"{} can detach, so it must be reattachable",
				capture.backend()
			);
		}
	}

	/// The capture is the directory itself, so it never detaches and there is
	/// never an exposure to put back.
	#[test]
	fn a_base_backup_exposes_nothing_separately() {
		assert!(
			!HeldCapture::BaseBackup {
				root: "/var/lib/bestool/held-source/x".into(),
			}
			.exposes_separately()
		);
	}

	/// Only the backends whose exposure is a mount this side can make. The others
	/// have to say so rather than silently do nothing and read as reattached.
	#[tokio::test]
	async fn reattaching_a_backend_without_a_mount_refuses() {
		let err = reattach(&HeldCapture::BaseBackup {
			root: std::env::temp_dir().join("bestool-hold-unit/x"),
		})
		.await
		.expect_err("a base backup exposes nothing to reattach");
		assert!(err.to_string().contains("not supported"), "{err}");
	}

	/// A record written before the device was kept still has to parse: dropping
	/// it would leave the hold unreadable and unreleasable at once.
	#[test]
	fn a_btrfs_capture_without_a_device_still_parses() {
		let json = br#"{
			"id": "x-20260814T054412Z",
			"backup_type": "tamanu-postgres",
			"held_at": "2026-08-14T11:02:00Z",
			"source": "/var/lib/bestool/held-source/x/18/main",
			"uploaded": true,
			"capture": {
				"backend": "btrfs",
				"toplevel_mount": "/run/t",
				"snapshot_path": "/run/t/bestool-held-x",
				"mount": "/var/lib/bestool/held-source/x"
			}
		}"#;
		let parsed = parse(json).unwrap();
		let HeldCapture::Btrfs { fsdev, .. } = parsed.capture else {
			panic!("expected a btrfs capture");
		};
		assert!(fsdev.is_none());
	}

	/// A shadow copy's junction substitutes a device path, and opening the
	/// junction itself opens that device rather than the root directory of the
	/// filesystem on it — so it reads empty however healthy the copy is, while
	/// every path through it resolves. Judging a hold by its junction therefore
	/// reports every held shadow copy detached, from the moment it is taken, with
	/// no reattach able to clear it. The state has to be read where a restore
	/// reads the capture.
	///
	/// Off Windows, where the shadow itself cannot be queried, the junction
	/// standing in for it is what makes this expressible as a unit test; the
	/// Windows side is covered end-to-end by `wmi_shadow_roundtrip`.
	#[cfg(not(windows))]
	#[tokio::test]
	async fn a_shadow_copy_hold_is_judged_where_the_restore_reads() {
		let scratch = tempfile::tempdir().unwrap();
		let junction = scratch.path().join("held").join("x");
		let source = junction.join("Program Files").join("PostgreSQL").join("12");
		std::fs::create_dir_all(&source).unwrap();

		let mut held = record(HeldCapture::Vss {
			shadow_id: "{deadbeef-0000-0000-0000-000000000000}".into(),
			junction: junction.clone(),
		});
		held.source = source;
		assert_eq!(capture_state(&held).await, CaptureState::Present);

		// The copy behind the junction stops serving the capture: what a restore
		// reads has gone, even though the junction is still standing.
		std::fs::remove_dir_all(junction.join("Program Files")).unwrap();
		assert_eq!(capture_state(&held).await, CaptureState::Detached);
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
