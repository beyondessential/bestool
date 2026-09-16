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
			hold::{CaptureState, HoldRecord, capture_state, list, load, records_dir, release},
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

/// A file rewritten at each point the capture and the live data need to stop
/// sharing storage, so that dropping the capture has something of its own to
/// return.
///
/// Without it a snapshot of a freshly-made cluster shares everything with the
/// live data, dropping it frees nothing measurable, and "released the capture"
/// cannot be told apart from "forgot the record".
const BALLAST: &str = "bestool-hold-e2e.ballast";

/// 64 MiB: comfortably above the noise in any of these stores, and small enough
/// that every fixture's filesystem can hold several copies of it.
///
/// Written only by the backends that measure a space delta. On the others it
/// would be paid for several times over for nothing — a base backup streams it
/// through `pg_basebackup`, walks it to size the restore, and copies it again
/// into staging, and a shadow copy grows its store by it — with no assertion
/// anywhere that reads it.
const BALLAST_BYTES: usize = 64 * 1024 * 1024;

/// How long to keep asking whether a dropped capture's space has come back.
///
/// btrfs unlinks a deleted subvolume at once but frees its extents on the
/// cleaner thread, which runs well after the command returns: around half a
/// minute for a capture this size on an unloaded machine. The poll gives up
/// early as soon as the space appears, so a generous budget costs nothing but
/// the time a genuine failure takes to report.
const RECLAIM_WITHIN: Duration = Duration::from_secs(120);

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

	/// Whether the capture has to hold storage of its own for this backend's
	/// release assertion to mean anything, and so whether the ballast is written.
	///
	/// False where the store is the machine's rather than the fixture's and
	/// nothing can be told from its size. [`Backend::capture_present`] carries the
	/// assertion alone there.
	///
	/// Declared separately from the readings below so that the two cannot be
	/// confused: a backend that measures nothing and a probe that failed would
	/// otherwise both come back as "no reading", and the second would quietly
	/// switch off the assertion it was meant to feed.
	fn needs_ballast(&self) -> bool {
		false
	}

	/// How much storage the capture holds that nothing else does, in bytes, for a
	/// filesystem that can account for it per capture.
	///
	/// This is the release claim made directly: storage nothing else references
	/// is storage that deleting the capture necessarily returns, so asking while
	/// the hold is still in place and then asserting the capture is gone says
	/// what a drop returned without ever measuring the filesystem around it —
	/// which on a copy-on-write filesystem is a number moved by metadata
	/// allocation, other writers, and when the cleaner last ran.
	async fn capture_exclusive_bytes(&self, record: &HoldRecord) -> Option<u64> {
		let _ = record;
		None
	}

	/// How much of the store the capture's space comes out of is in use, in
	/// whatever unit the backend reports it in — a percentage for a thin pool.
	/// Compared only against itself either side of a drop.
	///
	/// For a store that cannot account per capture, where the pool is the
	/// fixture's alone and a delta across the release is the best available
	/// reading.
	async fn store_in_use(&self) -> Option<f64> {
		None
	}

	/// The least a real release accounts for: exclusive bytes where the capture
	/// can be accounted for directly, otherwise the smallest fall in
	/// [`Backend::store_in_use`]. Given by the ballast either way.
	fn released_margin(&self) -> f64 {
		0.0
	}

	/// Quieten whatever else writes to the store, so that what the drop returns is
	/// measured against a filesystem only the drop is changing.
	async fn quiesce_store(&self) {}

	/// Wait for a store that frees asynchronously to finish doing so, so the
	/// measurement reads a settled filesystem rather than racing it.
	async fn settle_store(&self) {}

	/// What the store says about itself, for a failure to report rather than
	/// leaving the next reader to guess.
	async fn store_diagnostics(&self) -> String {
		String::new()
	}
}

/// Take a capture-only hold, use it, and release it — the operator's sequence,
/// asserted at every step.
async fn lifecycle<B: Backend>(backend: &B) {
	let data_dir = backend.data_dir().to_path_buf();
	let backup_type = backend.backup_type().to_owned();

	// Only the backends whose release assertion reads storage have anything to pin.
	let needs_ballast = backend.needs_ballast();
	let ballast_path = data_dir.join(BALLAST);
	let mut pin = |seed: u64| {
		if needs_ballast {
			write_ballast(&ballast_path, seed);
		}
	};

	// The value as it stands at the freeze, and the ballast the capture will pin.
	write(&data_dir.join(MARKER), FROZEN);
	pin(1);

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
	pin(2);

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

	// Give the capture sole claim on its ballast again before measuring what
	// dropping it returns.
	//
	// The restore copied the capture back into the live tree, and on a filesystem
	// that shares extents between a file and its copy the two now hold the same
	// blocks — so the capture pins nothing of its own, and a drop correctly frees
	// nothing. That is the filesystem behaving properly, not a hold failing to
	// release, and measuring across it would assert the opposite of what it looks
	// like. Rewriting the live copy breaks the sharing, leaving the capture the
	// only claim on what it froze.
	pin(3);

	// The probe that the drop is about to be judged by, asserted while the capture
	// is still there. A probe that answered "gone" for the wrong reason would make
	// the assertion after the drop pass without ever looking at the storage, and
	// that assertion is the whole point of the step.
	assert!(
		backend.capture_present(&held).await,
		"the {} probe cannot see hold {}'s capture while it is still held, so it \
		 proves nothing about the capture being gone after the drop",
		held.capture.backend(),
		held.id,
	);

	// Nothing but the drop should be moving the store while the drop is measured.
	// The cluster has already been asserted up and carrying the freeze, and on a
	// copy-on-write filesystem every write it makes allocates afresh, so leaving it
	// running would mix its allocations into the delta.
	backend.quiesce_store().await;

	// What the drop has to return, established while the capture is still there.
	//
	// A filesystem that accounts per capture answers outright, and that answer is
	// the claim: storage nothing else references is returned by deleting the thing
	// that holds it. One that cannot is read either side of the release instead.
	let exclusive = backend.capture_exclusive_bytes(&held).await;
	let before_drop = match exclusive {
		Some(_) => None,
		None => backend.store_in_use().await,
	};
	assert!(
		!needs_ballast || exclusive.is_some() || before_drop.is_some(),
		"the {} backend pins ballast for a release assertion, but can neither account \
		 for the capture's own storage nor read the store it comes out of, so nothing \
		 would have checked that dropping the hold returned anything",
		backend.expected_backend(),
	);
	if let Some(bytes) = exclusive {
		assert!(
			bytes as f64 >= backend.released_margin(),
			"hold {} holds only {bytes} bytes that nothing else does, short of the {} a \
			 released capture has to account for, so dropping it would return too little \
			 to tell from nothing.\n{}",
			held.id,
			backend.released_margin(),
			backend.store_diagnostics().await,
		);
	}

	// `bestool canopy hold drop`: the record, and the capture behind it.
	hold(HoldAction::Drop(DropArgs { id: held.id.clone() }))
		.await
		.expect("dropping the hold");

	// Let a backend that frees asynchronously finish before anything is read.
	backend.settle_store().await;

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
	if let Some(before) = before_drop
		&& let Err(seen) = store_fell_by(backend, before, backend.released_margin()).await
	{
		panic!(
			"dropping hold {} did not return the capture's space: the store was still \
			 within {} of {before} after {}s.\n  what the store read: {seen}\n{}",
			held.id,
			backend.released_margin(),
			RECLAIM_WITHIN.as_secs(),
			backend.store_diagnostics().await,
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
			// The staged path, which is what this lifecycle exercises: it stages a
			// copy of the capture and swaps it in, leaving the hold behind.
			in_place: false,
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

/// Release and forget the holds a backup type left behind. A lifecycle that
/// panicked leaves one, and the next run would find two and refuse to guess
/// which is its own.
///
/// The capture goes first, and through the same release the drop uses: a
/// stranded record names a subvolume, a logical volume, a shadow copy or a
/// staged tree that nothing else will ever name again, and removing the record
/// alone would strand it permanently — on the base-backup backend that is a
/// whole copy of the cluster sitting on the machine's own disk.
async fn clear_records(backup_type: &str) {
	for id in stranded_holds(backup_type) {
		if let Ok(record) = load(&id).await {
			let _ = release(&record.capture).await;
		}
		let _ = std::fs::remove_file(records_dir().join(format!("{id}.json")));
	}
}

/// The hold ids on the device belonging to a backup type.
///
/// Matched on the shape an id is minted with — the type, then a timestamp — and
/// not on the type as a bare prefix, which one type being another's prefix would
/// make ambiguous: `hold-e2e-vss` would otherwise claim `hold-e2e-vss-reboot`'s
/// holds and release a capture the other test is still using.
fn stranded_holds(backup_type: &str) -> Vec<String> {
	let Ok(entries) = std::fs::read_dir(records_dir()) else {
		return Vec::new();
	};
	entries
		.flatten()
		.filter_map(|entry| hold_id_of(&entry.file_name().to_string_lossy(), backup_type))
		.collect()
}

/// The hold id a record file names, if it belongs to this backup type.
fn hold_id_of(file_name: &str, backup_type: &str) -> Option<String> {
	let id = file_name.strip_suffix(".json")?;
	let stamp = id.strip_prefix(backup_type)?.strip_prefix('-')?;
	is_stamp(stamp).then(|| id.to_owned())
}

/// Whether this is the `%Y%m%dT%H%M%SZ` instant a hold id ends with.
fn is_stamp(text: &str) -> bool {
	let bytes = text.as_bytes();
	bytes.len() == 16
		&& bytes[..8].iter().all(u8::is_ascii_digit)
		&& bytes[8] == b'T'
		&& bytes[9..15].iter().all(u8::is_ascii_digit)
		&& bytes[15] == b'Z'
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::actions::canopy::backup::hold::mint_id;

	/// One backup type being another's prefix must not let the shorter one claim
	/// the longer one's holds: releasing a capture another test is still using
	/// would strand a live shadow copy with nothing left to name it.
	#[test]
	fn a_type_does_not_claim_a_longer_types_holds() {
		let at = "2026-09-15T05:37:06Z".parse().unwrap();
		let own = format!("{}.json", mint_id("hold-e2e-vss", at));
		let other = format!("{}.json", mint_id("hold-e2e-vss-reboot", at));

		assert_eq!(
			hold_id_of(&own, "hold-e2e-vss").as_deref(),
			Some("hold-e2e-vss-20260915T053706Z"),
		);
		assert_eq!(hold_id_of(&other, "hold-e2e-vss"), None);
		assert_eq!(
			hold_id_of(&other, "hold-e2e-vss-reboot").as_deref(),
			Some("hold-e2e-vss-reboot-20260915T053706Z"),
		);
	}

	/// Anything that is not a record of this type's is left alone: the directory
	/// is the device's, not the test's.
	#[test]
	fn only_this_types_record_files_match() {
		for name in [
			"hold-e2e-vss-20260915T053706Z",       // no extension
			"hold-e2e-vss-notastamp.json",         // not an instant
			"hold-e2e-vss.json",                   // no instant at all
			"tamanu-postgres-20260915T053706Z.json", // another type entirely
		] {
			assert_eq!(hold_id_of(name, "hold-e2e-vss"), None, "{name}");
		}
	}
}

/// A reading from a backend that reads its store either side of the release. One
/// that answered before the drop and not after is broken, not quiet: treating
/// that as "nothing to assert" would switch the release check off mid-way.
async fn read_store<B: Backend>(backend: &B, when: &str) -> f64 {
	backend.store_in_use().await.unwrap_or_else(|| {
		panic!(
			"the {} backend reads its store, but could not {when}",
			backend.expected_backend(),
		)
	})
}

/// Wait for the store's usage to fall by `margin`, allowing for a backend that
/// frees the space a beat after the command that released it returns.
///
/// On failure, reports what it actually saw: a delta that never arrived and one
/// that arrived too small are different problems, and so is a store that grew.
async fn store_fell_by<B: Backend>(
	backend: &B,
	before: f64,
	margin: f64,
) -> Result<(), String> {
	let deadline = std::time::Instant::now() + RECLAIM_WITHIN;
	let mut readings = 0_u32;
	let mut lowest = f64::MAX;
	loop {
		let now = read_store(backend, "after the drop").await;
		readings += 1;
		lowest = lowest.min(now);
		if before - now >= margin {
			return Ok(());
		}
		if std::time::Instant::now() >= deadline {
			return Err(format!(
				"{readings} readings, lowest {lowest}, last {now}, so it fell by {} at best",
				before - lowest,
			));
		}
		tokio::time::sleep(Duration::from_secs(1)).await;
	}
}

/// Write a file and get it onto the device before returning.
///
/// A capture taken underneath the filesystem — a thin-LVM snapshot, a shadow
/// copy — holds what has reached the block device, not what is sitting in the
/// page cache, so an unflushed marker could be missing from a capture that is
/// otherwise perfectly good. The directory goes too: a file created since the
/// last commit is not on the device until the entry naming it is.
fn write(path: &Path, contents: &[u8]) {
	use std::io::Write as _;

	let mut file = std::fs::File::create(path)
		.unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
	file.write_all(contents)
		.unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
	file.sync_all()
		.unwrap_or_else(|err| panic!("flushing {}: {err}", path.display()));
	sync_parent(path);
}

/// Flush the directory holding `path`: a file created since the last commit is
/// not on the device until the entry naming it is.
fn sync_parent(path: &Path) {
	if let Some(parent) = path.parent()
		&& let Ok(dir) = std::fs::File::open(parent)
	{
		let _ = dir.sync_all();
	}
}

/// Write the ballast: bytes that do not compress, so a filesystem that compresses
/// transparently still allocates what the ballast claims to. No two seeds share
/// an extent, which is the whole point of rewriting it rather than writing the
/// same bytes again.
///
/// Generated a chunk at a time rather than built whole in memory first: the
/// runner is already holding a postgres cluster, a loopback filesystem and a
/// staged restore copy, and the payload is the same on disk either way.
fn write_ballast(path: &Path, seed: u64) {
	use std::io::Write as _;

	const CHUNK: usize = 1024 * 1024;

	let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
	let mut chunk = Vec::with_capacity(CHUNK);
	let mut file = std::fs::File::create(path)
		.unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));

	let mut written = 0;
	while written < BALLAST_BYTES {
		let want = CHUNK.min(BALLAST_BYTES - written);
		chunk.clear();
		while chunk.len() < want {
			state ^= state << 13;
			state ^= state >> 7;
			state ^= state << 17;
			chunk.extend_from_slice(&state.to_le_bytes());
		}
		chunk.truncate(want);
		file.write_all(&chunk)
			.unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
		written += want;
	}
	file.sync_all()
		.unwrap_or_else(|err| panic!("flushing {}: {err}", path.display()));
	sync_parent(path);
}

#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod windows;
