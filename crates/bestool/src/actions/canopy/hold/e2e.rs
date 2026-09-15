//! The hold lifecycle an operator runs, exercised end to end on real storage.
//!
//! The pieces of a capture have had coverage for a while: a shadow copy is
//! created and read through, a detached subvolume is reattached. The *sequence*
//! an operator runs — take a hold, read the listing, restore from it, reattach
//! it, drop it — had none on any backend, which is how a hold that reported
//! detached from the moment it was taken shipped and stayed shipped.
//!
//! Each backend builds its own storage and a real postgres cluster on it, then
//! hands both to the single [`lifecycle`] driver below, so every backend is held
//! to the same sequence and the same assertions. The commands are driven through
//! their entry points with the argument structs clap would have built, so the
//! listing, the restore's capture-state gate, and the drop are the operator's
//! code paths rather than the library calls underneath them.
//!
//! Hold records and held captures live under fixed system directories, and the
//! storage is real, so every test here needs root (admin on Windows) and is
//! `#[ignore]`d for the per-backend CI jobs to run.

use std::{
	path::Path,
	time::Duration,
};

use miette::Result;

use super::{DropArgs, HoldAction, HoldArgs, ReattachArgs};
use crate::actions::{
	Context,
	canopy::{
		backup::{
			BackupArgs,
			hold::{CaptureState, HoldRecord, capture_state, list, load, records_dir},
		},
		restore::RestoreArgs,
	},
};

/// Written into the cluster before the capture: what a restore from the hold
/// must bring back.
const FROZEN: &[u8] = b"the value as it stood when the capture froze";

/// Written over it afterwards: what a restore from the hold must not bring back.
const LIVE: &[u8] = b"the value written after the capture froze";

/// The marker's name. Inside the cluster's data directory, so it travels with
/// every backend's capture without any backend needing to know about it.
const MARKER: &str = "bestool-hold-e2e.marker";

/// A file written before the capture and overwritten after it, so the capture
/// pins extents the live data no longer shares.
///
/// Without it a snapshot of a freshly-made cluster shares everything with the
/// live data, dropping it frees nothing measurable, and "released the capture"
/// cannot be told apart from "forgot the record".
const BALLAST: &str = "bestool-hold-e2e.ballast";

/// 64 MiB: comfortably above the noise in any of these stores, and small enough
/// that every fixture's filesystem can hold several copies of it.
const BALLAST_BYTES: usize = 64 * 1024 * 1024;

/// How long to keep asking whether a dropped capture's space has come back.
/// btrfs unlinks a deleted subvolume at once but frees its extents on the
/// cleaner thread, so the space returns shortly after the drop does.
const RECLAIM_WITHIN: Duration = Duration::from_secs(60);

/// What a backend's fixture supplies to the shared lifecycle.
trait Backend {
	/// The backup type. Unique per backend: a capture takes the same per-type
	/// lock an uploading run does, and the tests share one device.
	fn backup_type(&self) -> &str;

	/// The directory holding this backend's def.
	fn backups_dir(&self) -> &Path;

	/// The live cluster's data directory.
	fn data_dir(&self) -> &Path;

	/// The backend the hold record is expected to name.
	///
	/// Asserted rather than configured: a capture whose snapshot backend fails
	/// falls back to a base backup, which would otherwise leave the test quietly
	/// exercising a backend it does not mean to.
	fn expected_backend(&self) -> &'static str;

	/// Whether the capture is reached through something separate from itself, and
	/// so has an exposure for a reattach to be a no-op on.
	fn exposes_separately(&self) -> bool {
		true
	}

	/// Whether the cluster is up and accepting connections.
	async fn cluster_is_up(&self) -> bool;

	/// Whether the capture itself — the subvolume, the logical volume, the shadow
	/// copy, the staged tree — is still on the storage.
	///
	/// Asked after a drop, where the record going away proves nothing about the
	/// space behind it.
	async fn capture_present(&self, record: &HoldRecord) -> bool;

	/// How much of the store the capture's space comes out of is in use, in
	/// whatever unit the backend reports it in: bytes for a filesystem, a
	/// percentage for a thin pool. Compared only against itself either side of a
	/// drop.
	///
	/// `None` where the store is shared with the rest of the machine and a delta
	/// would be noise rather than evidence. [`Backend::capture_present`] carries
	/// the assertion alone there.
	async fn store_in_use(&self) -> Option<f64> {
		None
	}

	/// The smallest fall in [`Backend::store_in_use`] a real release shows, given
	/// the ballast the capture pins.
	fn released_margin(&self) -> f64 {
		0.0
	}
}

/// Take a capture-only hold, use it, and release it — the operator's sequence,
/// asserted at every step.
async fn lifecycle<B: Backend>(backend: &B) {
	let data_dir = backend.data_dir().to_path_buf();
	let backup_type = backend.backup_type().to_owned();

	// The value as it stands at the freeze, and the ballast the capture will pin.
	write(&data_dir.join(MARKER), FROZEN);
	write(&data_dir.join(BALLAST), &ballast(1));

	// `bestool canopy backup --type X --hold --no-upload`.
	backup(&backup_type, backend.backups_dir())
		.await
		.expect("taking a capture-only hold");

	let held = sole_hold(&backup_type).await;
	assert_eq!(
		held.capture.backend(),
		backend.expected_backend(),
		"hold {} was taken with the {} backend; the intended one must have failed and \
		 fallen back",
		held.id,
		held.capture.backend(),
	);
	assert!(!held.uploaded, "a capture-only hold reported itself uploaded");

	// The live cluster moves on. Everything from here distinguishes the capture
	// from the data as it now stands.
	write(&data_dir.join(MARKER), LIVE);
	write(&data_dir.join(BALLAST), &ballast(2));

	// `bestool canopy hold list`. A hold that reports detached from the moment it
	// is taken is not a rollback point, and is what this whole job exists to catch.
	hold(HoldAction::List).await.expect("listing held captures");
	assert_state(&held, CaptureState::Present, "just after it was taken").await;

	// And it reads where the record says a restore will read it.
	assert_marker(
		&held.source,
		FROZEN,
		"the held capture at the path its record names",
	);

	// `bestool canopy restore --type X --from-hold <id>`: the whole restore,
	// including the method stopping the cluster, swapping the tree into place, and
	// starting it again.
	restore(&backup_type, backend.backups_dir(), &held.id)
		.await
		.expect("restoring from the held capture");

	// What came back is the capture, not the live data it displaced.
	assert_marker(&data_dir, FROZEN, "the restored cluster");
	assert!(
		backend.cluster_is_up().await,
		"the cluster is not accepting connections after a restore from hold {}",
		held.id,
	);

	// Restoring copies the capture rather than moving it, so the same rollback
	// point is still there to be used again.
	let after = load(&held.id)
		.await
		.expect("the hold survived the restore that read it");
	assert_state(&after, CaptureState::Present, "after a restore from it").await;

	// `bestool canopy hold reattach` on a healthy hold.
	if backend.exposes_separately() {
		hold(HoldAction::Reattach(ReattachArgs { id: held.id.clone() }))
			.await
			.expect("reattaching a healthy hold");
		assert_state(&after, CaptureState::Present, "after reattaching it healthy").await;
		assert_marker(&after.source, FROZEN, "the held capture after a reattach");
	} else {
		// Nothing exposes it separately, so there is nothing to put back, and
		// saying so is better than reporting a no-op as a success.
		let err = hold(HoldAction::Reattach(ReattachArgs { id: held.id.clone() }))
			.await
			.expect_err("a capture with no separate exposure cannot be reattached");
		assert!(err.to_string().contains("not supported"), "{err}");
	}

	// `bestool canopy hold drop`: the record, and the capture behind it.
	let before_drop = backend.store_in_use().await;
	hold(HoldAction::Drop(DropArgs { id: held.id.clone() }))
		.await
		.expect("dropping the hold");

	assert!(
		load(&held.id).await.is_err(),
		"hold {} still has a record after being dropped",
		held.id,
	);
	assert!(
		!backend.capture_present(&held).await,
		"dropping hold {} removed its record but left the {} capture behind it on the storage",
		held.id,
		held.capture.backend(),
	);
	if let Some(before) = before_drop {
		assert!(
			store_fell_by(backend, before, backend.released_margin()).await,
			"dropping hold {} did not return the capture's space: the store was still \
			 within {} of {before} after {}s",
			held.id,
			backend.released_margin(),
			RECLAIM_WITHIN.as_secs(),
		);
	}
}

/// The state a hold reports, at the path a restore reads it.
async fn assert_state(record: &HoldRecord, want: CaptureState, when: &str) {
	let got = capture_state(record).await;
	assert_eq!(
		got, want,
		"hold {} reported {got:?} {when}; its capture reads at {}",
		record.id,
		record.source.display(),
	);
}

/// The marker as read out of a tree, named so a failure says which tree.
fn assert_marker(root: &Path, want: &[u8], what: &str) {
	let path = root.join(MARKER);
	let got = std::fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
	assert_eq!(
		String::from_utf8_lossy(&got),
		String::from_utf8_lossy(want),
		"{what} carries the wrong value ({})",
		path.display(),
	);
}

/// The one hold this backup type has. Filtered by type rather than taken from
/// the whole listing: the backends share one device's records directory, and a
/// test that panicked earlier may have left one of its own behind.
async fn sole_hold(backup_type: &str) -> HoldRecord {
	let mut mine: Vec<HoldRecord> = list()
		.await
		.expect("listing hold records")
		.into_iter()
		.filter(|record| record.backup_type == backup_type)
		.collect();
	assert_eq!(
		mine.len(),
		1,
		"expected exactly one hold for {backup_type}, found {}",
		mine.len(),
	);
	mine.remove(0)
}

async fn backup(backup_type: &str, backups_dir: &Path) -> Result<()> {
	super::super::backup::run(
		BackupArgs {
			backup_type: backup_type.to_owned(),
			config: None,
			backups_dir: Some(backups_dir.to_path_buf()),
			no_daemon: true,
			hold: true,
			no_upload: true,
		},
		Context::new(),
	)
	.await
}

async fn hold(action: HoldAction) -> Result<()> {
	super::run(HoldArgs { action }, Context::new()).await
}

async fn restore(backup_type: &str, backups_dir: &Path, hold_id: &str) -> Result<()> {
	super::super::restore::run(
		RestoreArgs {
			backup_type: backup_type.to_owned(),
			id: None,
			from_hold: Some(hold_id.to_owned()),
			target: None,
			clobber: true,
			no_followers: true,
			config: None,
			backups_dir: Some(backups_dir.to_path_buf()),
		},
		Context::new(),
	)
	.await
}

/// The refusal a restore from this hold gives, rendered for an assertion on what
/// it tells the operator. Only the VSS backend has a way to put a hold into a
/// state a restore refuses.
#[cfg(windows)]
async fn restore_err(record: &HoldRecord, backups_dir: &Path) -> String {
	restore(&record.backup_type, backups_dir, &record.id)
		.await
		.expect_err("a restore from a capture that cannot be read has to refuse")
		.to_string()
}

/// Forget the hold records a backup type left behind. A lifecycle that panicked
/// leaves one, and the next run would find two and refuse to guess which is its
/// own. The capture behind a stranded record goes with the storage the fixture
/// rebuilds around it; on Windows, where the storage is the machine's, the
/// shadow copy is left for the runner to take with it.
fn clear_records(backup_type: &str) {
	let prefix = format!("{backup_type}-");
	let Ok(entries) = std::fs::read_dir(records_dir()) else {
		return;
	};
	for entry in entries.flatten() {
		if entry.file_name().to_string_lossy().starts_with(&prefix) {
			let _ = std::fs::remove_file(entry.path());
		}
	}
}

/// Whether the store's usage fell by `margin`, allowing for a backend that frees
/// the space a beat after the command that released it returns.
async fn store_fell_by<B: Backend>(backend: &B, before: f64, margin: f64) -> bool {
	let deadline = std::time::Instant::now() + RECLAIM_WITHIN;
	loop {
		if let Some(now) = backend.store_in_use().await
			&& before - now >= margin
		{
			return true;
		}
		if std::time::Instant::now() >= deadline {
			return false;
		}
		tokio::time::sleep(Duration::from_secs(1)).await;
	}
}

fn write(path: &Path, contents: &[u8]) {
	std::fs::write(path, contents)
		.unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
}

/// Ballast bytes that do not compress, so a filesystem that compresses
/// transparently still allocates what the ballast claims to. Two different seeds
/// share no extents, which is the whole point of writing it twice.
fn ballast(seed: u64) -> Vec<u8> {
	let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
	let mut out = Vec::with_capacity(BALLAST_BYTES);
	while out.len() < BALLAST_BYTES {
		state ^= state << 13;
		state ^= state >> 7;
		state ^= state << 17;
		out.extend_from_slice(&state.to_le_bytes());
	}
	out
}

#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod windows;
