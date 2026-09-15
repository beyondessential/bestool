//! Laying a held capture back down over the live tree, without staging a copy.
//!
//! This is the part every method shares: work out what diverged, refuse if there
//! is not room for it, and write it. What surrounds it — stopping a service,
//! making a half-restored cluster unstartable — is the method's own, because
//! only the method knows what "startable" means for it.
//!
//! The sequence is idempotent. Running it again over a tree it half-finished
//! converges on the same result, which is what makes a failed in-place restore
//! resumable: the hold survives (reads consume no copy-on-write space), so the
//! documented recovery is to run the same command again.

use std::path::{Path, PathBuf};

use miette::{Result, bail};
use tracing::info;

use super::{basis, space, sync};
use crate::actions::canopy::backup::hold::HoldRecord;

/// One tree to roll back onto one capture.
pub struct Job<'a> {
	/// The hold being restored from, which names the backend and what it noted
	/// at the freeze.
	pub record: &'a HoldRecord,
	/// The capture's root, as the restore reads it.
	pub capture: &'a Path,
	/// The live tree the capture is laid back over.
	pub live: &'a Path,
	/// Paths, relative to both roots, the sync must not touch. The interlock
	/// keeps its own file out this way, so it can be written back last.
	pub skip: Vec<PathBuf>,
}

/// What the restore did, for the operator-facing report.
#[derive(Debug, Default)]
pub struct Summary {
	pub copied: usize,
	pub removed: usize,
	pub bytes: u64,
	/// Whether the filesystem named the diverged set, or the trees were compared.
	pub from_filesystem: bool,
}

/// Compare, gate on space, and write the divergence.
pub async fn run(job: Job<'_>) -> Result<Summary> {
	let Job {
		record,
		capture,
		live,
		skip,
	} = job;

	// An unreadable or empty capture would turn a rollback into an erasure, and
	// unlike a staged restore there is no `.old` to put back.
	sync::ensure_readable(capture).await?;

	if !live.exists() {
		// Nothing to diverge from: everything is a copy, and the distinction
		// between in place and staged does not arise.
		info!(
			live = %live.display(),
			"the destination is not there, so the capture is laid down whole"
		);
	}

	let delta = sync::compare(capture, live, &skip).await?;
	let basis = basis::resolve(record, live).await;

	// The walk settles every structural question; the basis, or failing that a
	// content comparison, settles the entries it could not.
	let also_copy = if basis.is_named() {
		delta
			.undecided
			.iter()
			.filter(|(rel, _, _)| basis.names(rel))
			.map(|(rel, _, _)| rel.clone())
			.collect()
	} else {
		sync::decide_undecided(capture, live, delta.undecided.clone()).await?
	};

	// A same-size entry costs its own size to write, and frees nothing, so it
	// lands on both sides of the ledger before the gate sees it.
	let mut delta = delta;
	for rel in &also_copy {
		let len = capture
			.join(rel)
			.symlink_metadata()
			.map(|meta| meta.len())
			.unwrap_or(0);
		delta.copy_bytes = delta.copy_bytes.saturating_add(len);
		delta.displaced_bytes = delta.displaced_bytes.saturating_add(len);
	}

	// Where the backend can size the divergence exactly and cheaply, that beats
	// the walk's estimate; it can only raise the bar, never lower it.
	if let Some(exact) = basis::divergence_bytes(record, live).await {
		delta.copy_bytes = delta.copy_bytes.max(exact);
	}

	let store = space::cow_store(record).await;
	space::ensure_room(live, &delta, &store).await?;

	let copy: Vec<PathBuf> = delta.copy.iter().cloned().chain(also_copy).collect();
	// Whether there is work, not whether the walk found anything to think about:
	// a tree that already matches still has every same-size entry to consider,
	// and reporting that as a restore would hide a no-op.
	if copy.is_empty() && !delta.has_work() {
		info!("the live tree already matches the capture; nothing to write");
		return Ok(Summary {
			from_filesystem: basis.is_named(),
			..Default::default()
		});
	}

	info!(
		copying = copy.len(),
		removing = delta.remove.len(),
		bytes = delta.copy_bytes,
		"laying the divergence down over the live tree",
	);
	sync::apply(capture, live, &delta, &copy).await?;

	Ok(Summary {
		copied: copy.len(),
		removed: delta.remove.len(),
		bytes: delta.copy_bytes,
		from_filesystem: basis.is_named(),
	})
}

/// The marker an in-place restore leaves in the tree while it is in flight.
///
/// Its presence is what tells a later run — or a person — that the tree is
/// neither the state it was in nor the state that was captured. It is written
/// before the first write and removed after the last.
pub struct Interlock {
	marker: PathBuf,
}

/// The marker's name. Inside the tree being restored, because that is where
/// anyone looking at a suspect cluster looks, and because it then travels with
/// the tree rather than with the host.
const MARKER: &str = ".bestool-in-place-restore";

impl Interlock {
	/// The marker's path within a tree, for a caller that has to keep it out of
	/// the sync.
	pub fn marker_in(live: &Path) -> PathBuf {
		live.join(MARKER)
	}

	/// Whether a tree is mid-restore.
	pub async fn held_by(live: &Path) -> bool {
		tokio::fs::metadata(Self::marker_in(live)).await.is_ok()
	}

	/// Mark the tree as mid-restore.
	pub async fn engage(live: &Path, record: &HoldRecord) -> Result<Self> {
		let marker = Self::marker_in(live);
		let resuming = tokio::fs::metadata(&marker).await.is_ok();
		let note = format!(
			"This directory is part-way through an in-place restore and is NOT usable.\n\
			 \n\
			 hold:    {}\n\
			 capture: {}\n\
			 frozen:  {}\n\
			 started: {}\n\
			 \n\
			 It is neither the state it was in before nor the state that was captured.\n\
			 The hold is untouched, so run the same restore again to finish it. Do not\n\
			 start the service against this directory until that succeeds and removes\n\
			 this file.\n",
			record.id,
			record.source.display(),
			record
				.taken_at
				.map_or_else(|| "unknown".to_owned(), |at| at.to_string()),
			jiff::Timestamp::now(),
		);
		if let Some(parent) = marker.parent() {
			tokio::fs::create_dir_all(parent).await.ok();
		}
		tokio::fs::write(&marker, note)
			.await
			.map_err(|err| miette::miette!("marking {} as mid-restore: {err}", marker.display()))?;
		if resuming {
			info!(
				live = %live.display(),
				"resuming an in-place restore this directory was left part-way through",
			);
		}
		Ok(Self { marker })
	}

	/// Release the tree: the restore finished and it is the captured state.
	pub async fn release(self) -> Result<()> {
		tokio::fs::remove_file(&self.marker).await.map_err(|err| {
			miette::miette!(
				"the restore finished but {} could not be removed, and while it is \
				 there the cluster is refused a start: {err}",
				self.marker.display()
			)
		})
	}

	/// Refuse to go on while a tree is mid-restore.
	pub async fn ensure_clear(live: &Path) -> Result<()> {
		if Self::held_by(live).await {
			bail!(
				"{} is part-way through an in-place restore, so it is neither the state \
				 it was in nor the state that was captured; finish the restore (run it \
				 again — the hold is untouched) before starting anything against it",
				live.display()
			);
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::actions::canopy::backup::hold::HeldCapture;

	fn record(source: &Path) -> HoldRecord {
		HoldRecord {
			id: "tamanu-postgres-20260915T052531Z".into(),
			backup_type: "tamanu-postgres".into(),
			taken_at: Some("2026-09-15T05:25:31Z".parse().unwrap()),
			held_at: "2026-09-15T05:25:40Z".parse().unwrap(),
			source: source.to_path_buf(),
			uploaded: false,
			capture: HeldCapture::BaseBackup {
				root: source.to_path_buf(),
			},
			diverged_since: None,
		}
	}

	fn write(root: &Path, rel: &str, contents: &str) {
		let path = root.join(rel);
		std::fs::create_dir_all(path.parent().unwrap()).unwrap();
		std::fs::write(path, contents).unwrap();
	}

	#[tokio::test]
	async fn rolls_the_live_tree_back_to_the_capture() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let live = tmp.path().join("live");
		write(&capture, "PG_VERSION", "16");
		write(&capture, "base/1/2345", "as it was at the freeze");
		write(&live, "PG_VERSION", "16");
		write(&live, "base/1/2345", "written after the freeze!!!");
		write(&live, "base/1/9999", "created after the freeze");

		let record = record(&capture);
		let summary = run(Job {
			record: &record,
			capture: &capture,
			live: &live,
			skip: Vec::new(),
		})
		.await
		.unwrap();

		assert_eq!(
			std::fs::read_to_string(live.join("base/1/2345")).unwrap(),
			"as it was at the freeze"
		);
		assert!(!live.join("base/1/9999").exists());
		assert!(!summary.from_filesystem, "no basis was recorded for this capture");
	}

	#[tokio::test]
	async fn a_tree_that_already_matches_is_reported_as_no_work() {
		// Every entry is present on both sides at the same size, so the walk has
		// plenty to consider and nothing to do. Reporting that as a restore would
		// hide a no-op behind a count.
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let live = tmp.path().join("live");
		write(&capture, "PG_VERSION", "16");
		write(&capture, "base/1/2345", "the captured page");
		write(&live, "PG_VERSION", "16");
		write(&live, "base/1/2345", "the captured page");

		let record = record(&capture);
		let summary = run(Job {
			record: &record,
			capture: &capture,
			live: &live,
			skip: Vec::new(),
		})
		.await
		.unwrap();

		assert_eq!(summary.copied, 0);
		assert_eq!(summary.removed, 0);
		assert_eq!(summary.bytes, 0);
	}

	#[tokio::test]
	async fn an_interrupted_restore_is_resumed_by_running_it_again() {
		// The recovery the mode documents: the hold is untouched by a failure, and
		// the sync converges, so the same command finishes the job.
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let live = tmp.path().join("live");
		write(&capture, "base/1/2345", "as at the freeze");
		write(&capture, "base/1/3456", "also as at the freeze");
		write(&live, "base/1/2345", "diverged since then");
		write(&live, "base/1/9999", "created since then");
		let record = record(&capture);

		// Stand in for a failure partway: the interlock is engaged and only part of
		// the divergence is down.
		let interlock = Interlock::engage(&live, &record).await.unwrap();
		std::fs::write(live.join("base/1/2345"), "as at the freeze").unwrap();
		drop(interlock);
		assert!(Interlock::held_by(&live).await, "the tree is still marked mid-restore");

		let interlock = Interlock::engage(&live, &record).await.unwrap();
		run(Job {
			record: &record,
			capture: &capture,
			live: &live,
			skip: vec![PathBuf::from(
				Interlock::marker_in(Path::new("")).file_name().unwrap(),
			)],
		})
		.await
		.unwrap();
		interlock.release().await.unwrap();

		assert_eq!(
			std::fs::read_to_string(live.join("base/1/3456")).unwrap(),
			"also as at the freeze"
		);
		assert!(!live.join("base/1/9999").exists());
		Interlock::ensure_clear(&live).await.unwrap();
	}

	#[tokio::test]
	async fn an_empty_capture_is_refused_before_anything_is_written() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let live = tmp.path().join("live");
		std::fs::create_dir_all(&capture).unwrap();
		write(&live, "PG_VERSION", "16");

		let record = record(&capture);
		let err = run(Job {
			record: &record,
			capture: &capture,
			live: &live,
			skip: Vec::new(),
		})
		.await
		.unwrap_err()
		.to_string();

		assert!(err.contains("erase the cluster"), "got: {err}");
		assert!(
			live.join("PG_VERSION").exists(),
			"the live tree must be untouched when the capture is refused"
		);
	}

	#[tokio::test]
	async fn a_skipped_path_is_left_alone_on_both_sides() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let live = tmp.path().join("live");
		write(&capture, "PG_VERSION", "16");
		write(&capture, "data", "as it was at the freeze");
		write(&live, "data", "diverged");

		let record = record(&capture);
		run(Job {
			record: &record,
			capture: &capture,
			live: &live,
			skip: vec![PathBuf::from("PG_VERSION")],
		})
		.await
		.unwrap();

		assert_eq!(
			std::fs::read_to_string(live.join("data")).unwrap(),
			"as it was at the freeze"
		);
		assert!(
			!live.join("PG_VERSION").exists(),
			"the interlock's own file is written back by the interlock, not the sync"
		);
	}

	#[tokio::test]
	async fn the_interlock_refuses_a_start_while_it_is_engaged_and_permits_one_after() {
		let tmp = tempfile::tempdir().unwrap();
		let live = tmp.path().join("live");
		std::fs::create_dir_all(&live).unwrap();
		let record = record(&live);

		Interlock::ensure_clear(&live).await.unwrap();

		let interlock = Interlock::engage(&live, &record).await.unwrap();
		assert!(Interlock::held_by(&live).await);
		let err = Interlock::ensure_clear(&live).await.unwrap_err().to_string();
		assert!(err.contains("part-way through"), "got: {err}");

		interlock.release().await.unwrap();
		Interlock::ensure_clear(&live).await.unwrap();
	}

	#[tokio::test]
	async fn the_marker_names_the_hold_to_resume_from() {
		let tmp = tempfile::tempdir().unwrap();
		let live = tmp.path().join("live");
		std::fs::create_dir_all(&live).unwrap();
		let record = record(&live);

		let _interlock = Interlock::engage(&live, &record).await.unwrap();
		let note = std::fs::read_to_string(Interlock::marker_in(&live)).unwrap();
		assert!(note.contains("tamanu-postgres-20260915T052531Z"), "got: {note}");
		assert!(note.contains("2026-09-15T05:25:31Z"), "got: {note}");
	}
}
