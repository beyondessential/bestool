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

use super::{basis, room, sync};
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

/// A checked plan: what the restore will write, and the room for it confirmed.
///
/// Kept separate from laying it down so a caller can find out whether a restore
/// is possible *before* it makes the data unusable. Everything that can refuse —
/// an unreadable capture, a filesystem or copy-on-write store without room —
/// happens here, and nothing here touches the tree being restored.
///
/// It is not side-effect free, though: sizing the divergence on a thin pool
/// reserves and releases a pool-wide metadata snapshot, which is why it is not
/// asked for at all when the walk has already found nothing to write.
pub struct Planned {
	capture: PathBuf,
	live: PathBuf,
	delta: sync::Delta,
	from_filesystem: bool,
}

impl Planned {
	/// Whether there is anything to write or remove.
	pub fn has_work(&self) -> bool {
		self.delta.has_work()
	}
}

/// Work out what the restore would do, and confirm there is room for it.
pub async fn plan(job: Job<'_>) -> Result<Planned> {
	let Job {
		record,
		capture,
		live,
		mut skip,
	} = job;

	// The marker is the caller's own bookkeeping rather than captured state, so
	// the sync must never see it. Added here rather than left to each caller,
	// because a caller that forgets fails in exactly the way the interlock
	// exists to prevent: the marker is removed as a post-freeze file, and the
	// tree stops saying it is mid-restore.
	let marker = PathBuf::from(Interlock::marker_name());
	if !skip.contains(&marker) {
		skip.push(marker);
	}

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

	// The basis is resolved before the walk so the walk can settle every entry as
	// it meets it, rather than accumulating one row per same-size entry — on a
	// tree this size that list is the largest thing in memory and it is not the
	// delta, it is the whole cluster.
	let basis = basis::resolve(record, live).await;
	let from_filesystem = basis.is_named();
	let delta = sync::compare(capture, live, &skip, basis.into_decision()).await?;

	let planned = Planned {
		capture: capture.to_path_buf(),
		live: live.to_path_buf(),
		delta,
		from_filesystem,
	};

	// Nothing to write needs no room, and asking anyway is not free: sizing the
	// divergence on a thin pool reserves a metadata snapshot across the whole
	// pool. This is the converged case a resumed restore hits every time.
	if !planned.has_work() {
		return Ok(planned);
	}

	// Where the backend can size the divergence exactly and cheaply, that is a
	// better number than the walk's — but only for the copy-on-write store. It
	// is a count of differing *blocks*, which says nothing about how much the
	// tree grows, and folding it into the delta would raise the filesystem
	// requirement too and could refuse a restore that fits.
	let cow_bytes = basis::divergence_bytes(record, live).await;
	let store = room::cow_store(record).await;
	room::ensure_room(live, &planned.delta, &store, cow_bytes).await?;

	Ok(planned)
}

/// Write the divergence, which is the point of no return.
pub async fn lay_down(planned: Planned) -> Result<Summary> {
	let Planned {
		capture,
		live,
		delta,
		from_filesystem,
	} = planned;

	if !delta.has_work() {
		info!("the live tree already matches the capture; nothing to write");
		return Ok(Summary {
			from_filesystem,
			..Default::default()
		});
	}

	info!(
		copying = delta.copy.len(),
		removing = delta.remove.len(),
		bytes = delta.copy_bytes,
		"laying the divergence down over the live tree",
	);
	sync::apply(&capture, &live, &delta).await?;

	Ok(Summary {
		copied: delta.copy.len(),
		removed: delta.remove.len(),
		bytes: delta.copy_bytes,
		from_filesystem,
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

/// The line of the marker naming the hold whose restore is in flight.
const HOLD_LINE: &str = "hold:";

impl Interlock {
	/// The marker's file name, for a caller that has to keep it out of the sync.
	pub fn marker_name() -> &'static str {
		MARKER
	}

	/// The marker's path within a tree.
	pub fn marker_in(live: &Path) -> PathBuf {
		live.join(MARKER)
	}

	/// Whether a tree is mid-restore.
	///
	/// On the link itself, not what it points at: a marker that is a symlink is
	/// not one this wrote, and following it would report on some other file.
	pub async fn held_by(live: &Path) -> bool {
		tokio::fs::symlink_metadata(Self::marker_in(live))
			.await
			.is_ok_and(|meta| meta.is_file())
	}

	/// Whether a tree was left part-way through a restore *from this same hold*.
	///
	/// A caller treats that as consent already given: finishing an interrupted
	/// restore is the documented recovery, and it should not need a confirmation
	/// the first attempt did not. Only for the same hold, though — the marker is
	/// an ordinary file in a directory the service account can write, so a stale
	/// one from an unrelated restore, or one someone dropped there, must not
	/// stand in for an operator saying yes to overwriting live data.
	pub async fn resumes(live: &Path, record: &HoldRecord) -> bool {
		// The same file class `held_by` recognises, and read without following a
		// link: this is the one check that waives the overwrite confirmation, so a
		// symlink planted at the marker's path must not be able to point it at any
		// file that happens to contain a matching `hold:` line. Hold ids are
		// `<type>-<timestamp>` and enumerable from a hold listing, so that is not a
		// hard thing to arrange for anyone who can write the data directory.
		if !Self::held_by(live).await {
			return false;
		}
		let Ok(note) = read_regular_file(Self::marker_in(live)).await else {
			return false;
		};
		marked_hold(&note).is_some_and(|held| held == record.id)
	}

	/// Mark the tree as mid-restore.
	pub async fn engage(live: &Path, record: &HoldRecord) -> Result<Self> {
		let marker = Self::marker_in(live);
		let resuming = tokio::fs::metadata(&marker).await.is_ok();
		let note = format!(
			"This directory is part-way through an in-place restore and is NOT usable.\n\
			 \n\
			 hold: {}\n\
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
		write_marker(&marker, note).await?;
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

/// Read a file, refusing to traverse a link standing in for it.
///
/// Also bounds what is read: the marker is a short note, and a link pointed at
/// something endless would otherwise be pulled into memory whole.
async fn read_regular_file(path: PathBuf) -> std::io::Result<String> {
	/// Generous for a note of a few lines, small enough that a wrong file costs
	/// nothing to reject.
	const MOST: u64 = 64 * 1024;

	tokio::task::spawn_blocking(move || {
		use std::io::Read as _;

		let mut file = open_no_follow(&path, std::fs::OpenOptions::new().read(true))?;
		let mut text = String::new();
		file.by_ref().take(MOST).read_to_string(&mut text)?;
		Ok(text)
	})
	.await
	.map_err(std::io::Error::other)?
}

/// Open `path` without traversing a symlink or reparse point at the final
/// component, on either platform.
fn open_no_follow(path: &Path, options: &mut std::fs::OpenOptions) -> std::io::Result<std::fs::File> {
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt as _;
		options.custom_flags(libc::O_NOFOLLOW);
	}
	#[cfg(windows)]
	{
		use std::os::windows::fs::OpenOptionsExt as _;
		// Opens the reparse point itself rather than what it names, so a junction
		// or symlink planted here is opened (and then rejected) rather than
		// followed — the same guarantee `O_NOFOLLOW` gives on Unix. Windows is
		// where this matters most: the VSS backend is the Windows in-place
		// backend.
		const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
		options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
	}
	options.open(path)
}

/// Write the marker, refusing to follow anything standing where it belongs.
///
/// The marker sits inside the data directory, which the service account can
/// write, while the restore runs elevated. An ordinary write follows a symlink,
/// so a planted one pointing at a root-owned file would see that file truncated
/// and overwritten with this note. Anything there that is not a regular file is
/// removed first, and the open refuses to traverse a link even if one is put
/// back in between.
async fn write_marker(marker: &Path, note: String) -> Result<()> {
	let marker = marker.to_path_buf();
	tokio::task::spawn_blocking(move || {
		use std::io::Write as _;

		match marker.symlink_metadata() {
			Ok(meta) if !meta.is_file() => {
				let removed = if meta.is_dir() {
					std::fs::remove_dir_all(&marker)
				} else {
					// Not `remove_file`: on Windows a directory link needs
					// `RemoveDirectoryW`, which is what this works out.
					super::sync::remove_entry(&marker, false)
				};
				removed.map_err(|err| {
					miette::miette!("clearing {} to mark it mid-restore: {err}", marker.display())
				})?;
			}
			_ => {}
		}

		// The open itself refuses to traverse a link, which closes the window
		// between the check above and here.
		let mut options = std::fs::OpenOptions::new();
		options.write(true).create(true).truncate(true);
		let mut file = open_no_follow(&marker, &mut options).map_err(|err| {
			miette::miette!("marking {} as mid-restore: {err}", marker.display())
		})?;
		file.write_all(note.as_bytes()).map_err(|err| {
			miette::miette!("marking {} as mid-restore: {err}", marker.display())
		})
	})
	.await
	.map_err(|err| miette::miette!("joining the marker write: {err}"))?
}

/// The hold id a marker names, if it names one.
///
/// The marker is written for a person to read, so this reads the one line that
/// has to be machine-readable and ignores the prose around it.
fn marked_hold(note: &str) -> Option<&str> {
	note.lines()
		.find_map(|line| line.trim().strip_prefix(HOLD_LINE))
		.map(str::trim)
		.filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::actions::canopy::backup::hold::HeldCapture;

	/// Plan and lay down in one go. The two are separate in the real callers so
	/// that a refusal can leave the tree untouched; a test that expects to get
	/// through both does not care.
	async fn run(job: Job<'_>) -> Result<Summary> {
		lay_down(plan(job).await?).await
	}

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

	/// The case that made a resumed restore unrecoverable: an attempt interrupted
	/// after its last copy leaves nothing for the next walk to find, because the
	/// interlock's own files are skipped. Deciding what to do on the delta alone
	/// would then never put the tree back together, on every re-run.
	#[tokio::test]
	async fn a_converged_resume_still_has_a_marker_to_clear() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let live = tmp.path().join("live");
		write(&capture, "f", "as at the freeze");
		write(&live, "f", "as at the freeze");
		let record = record(&capture);

		// Stand in for an attempt that copied everything and then died.
		let interlock = Interlock::engage(&live, &record).await.unwrap();
		drop(interlock);

		let planned = plan(Job {
			record: &record,
			capture: &capture,
			live: &live,
			skip: Vec::new(),
		})
		.await
		.unwrap();

		assert!(!planned.has_work(), "the tree already matches the capture");
		assert!(
			Interlock::resumes(&live, &record).await,
			"and yet it is still marked mid-restore, which is what has to be acted on"
		);
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
	async fn a_marker_from_this_hold_is_a_resume() {
		let tmp = tempfile::tempdir().unwrap();
		let live = tmp.path().join("live");
		std::fs::create_dir_all(&live).unwrap();
		let record = record(&live);

		assert!(!Interlock::resumes(&live, &record).await, "no marker, no resume");
		let _interlock = Interlock::engage(&live, &record).await.unwrap();
		assert!(Interlock::resumes(&live, &record).await);
	}

	/// The marker stands in for a confirmation the operator already gave, so it
	/// only counts for the restore they gave it for. It is an ordinary file in a
	/// directory the service account can write, and any aborted restore leaves
	/// one behind — treating a stale or planted marker as consent would let a
	/// restore from any hold overwrite the live cluster unprompted.
	#[tokio::test]
	async fn a_marker_from_another_hold_is_not_consent() {
		let tmp = tempfile::tempdir().unwrap();
		let live = tmp.path().join("live");
		std::fs::create_dir_all(&live).unwrap();
		let mine = record(&live);

		let _interlock = Interlock::engage(&live, &mine).await.unwrap();

		let mut other = record(&live);
		other.id = "tamanu-postgres-20260101T000000Z".into();
		assert!(Interlock::held_by(&live).await, "the tree is mid-restore");
		assert!(
			!Interlock::resumes(&live, &other).await,
			"a marker for a different hold must not stand in for confirmation"
		);
	}

	#[tokio::test]
	async fn a_marker_naming_no_hold_is_not_consent() {
		let tmp = tempfile::tempdir().unwrap();
		let live = tmp.path().join("live");
		std::fs::create_dir_all(&live).unwrap();
		let record = record(&live);
		// Anyone with write access to the data directory can leave a file here.
		std::fs::write(Interlock::marker_in(&live), "planted by someone else
").unwrap();

		assert!(Interlock::held_by(&live).await);
		assert!(!Interlock::resumes(&live, &record).await);
	}

	/// A planted link must not be able to forge the consent that waives the
	/// overwrite confirmation, whatever it points at.
	#[cfg(unix)]
	#[tokio::test]
	async fn a_symlink_naming_the_hold_is_not_consent() {
		let tmp = tempfile::tempdir().unwrap();
		let live = tmp.path().join("live");
		std::fs::create_dir_all(&live).unwrap();
		let record = record(&live);

		// Any file whose contents happen to carry the hold's id would do; the id is
		// `<type>-<timestamp>` and shows up in a hold listing.
		let elsewhere = tmp.path().join("bait");
		std::fs::write(&elsewhere, format!("hold: {}\n", record.id)).unwrap();
		std::os::unix::fs::symlink(&elsewhere, Interlock::marker_in(&live)).unwrap();

		assert!(
			!Interlock::resumes(&live, &record).await,
			"a link is not a marker this wrote, so it cannot stand in for a confirmation"
		);
	}

	/// The marker lives in a directory the service account can write while the
	/// restore runs elevated, so a link planted where it belongs must not be
	/// followed: doing so would truncate and overwrite whatever it points at.
	#[cfg(unix)]
	#[tokio::test]
	async fn a_symlink_standing_in_for_the_marker_is_not_written_through() {
		let tmp = tempfile::tempdir().unwrap();
		let live = tmp.path().join("live");
		std::fs::create_dir_all(&live).unwrap();
		let elsewhere = tmp.path().join("precious");
		std::fs::write(&elsewhere, "must not be touched").unwrap();
		std::os::unix::fs::symlink(&elsewhere, Interlock::marker_in(&live)).unwrap();

		let record = record(&live);
		assert!(!Interlock::held_by(&live).await, "a link is not a marker this wrote");

		let interlock = Interlock::engage(&live, &record).await.unwrap();

		assert_eq!(
			std::fs::read_to_string(&elsewhere).unwrap(),
			"must not be touched",
			"the file the link pointed at must be untouched"
		);
		assert!(Interlock::marker_in(&live).symlink_metadata().unwrap().is_file());
		assert!(Interlock::resumes(&live, &record).await);
		interlock.release().await.unwrap();
	}

	#[test]
	fn reads_the_hold_out_of_a_marker_and_nothing_else_out_of_the_prose() {
		let note = "This directory is part-way through an in-place restore and is NOT usable.\n\
		            \n\
		            hold: tamanu-postgres-20260915T052531Z\n\
		            capture: /var/lib/bestool/held-source/x\n";
		assert_eq!(marked_hold(note), Some("tamanu-postgres-20260915T052531Z"));
		assert_eq!(marked_hold("no hold line here"), None);
		assert_eq!(marked_hold("hold:   \n"), None);
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
