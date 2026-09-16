//! Comparing a held capture against the live tree, and applying the difference.
//!
//! The expensive part of a restore is reading file *contents*: on the host that
//! motivated this mode, 732 GiB on each side. Reading *metadata* — `readdir`
//! plus `stat` — is cheap even on a tree that large, and it already answers
//! every structural question: what is only in the capture (copy), what is only
//! in the live tree (delete), what changed kind, and what changed size.
//!
//! That leaves exactly one undecided case: an entry present in both at the same
//! size. A [diff basis](super::basis) decides that case and nothing else, which
//! is why a basis is never load-bearing for correctness — an absent, stale, or
//! wrapped one degrades to [`Decide::Compare`] and the restore is still
//! complete.
//!
//! Deletions come from the walk rather than from the basis. That matters
//! because the backends' change lists do not all report them: `btrfs subvolume
//! find-new` cannot, since a deleted file leaves no inode carrying a newer
//! generation.

use std::{
	collections::{BTreeMap, BTreeSet},
	path::{Path, PathBuf},
};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use tracing::{debug, info};

/// How much of a file is read at a time when comparing it against its captured
/// self.
const COMPARE_CHUNK: usize = 1024 * 1024;

/// How the walk settles an entry present in both trees at the same size, which
/// is the one question metadata cannot answer.
pub enum Decide {
	/// The filesystem named the diverged set, so an entry is copied exactly when
	/// it is in it and neither side is read.
	Named(BTreeSet<PathBuf>),
	/// Nothing to go on, so the two sides are compared: an entry whose
	/// modification time also matches is left alone, and any other is read.
	///
	/// The skip on a matching modification time is the degraded part — a file
	/// that diverges without changing size or mtime is kept — and is acceptable
	/// only because this is the fallback.
	Compare,
}

/// One entry the live tree has and the capture does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
	/// Where it is, relative to the tree roots.
	pub rel: PathBuf,
	/// Whether it is a directory in its own right — not a symlink to one, which
	/// is unlinked rather than descended into. The walk already knows, so laying
	/// the delta down does not have to ask again.
	pub is_dir: bool,
}

/// The difference between a capture and the live tree it rolls back onto.
#[derive(Debug, Default)]
pub struct Delta {
	/// Directories to create, parents before children.
	pub mkdir: Vec<PathBuf>,
	/// Directories present on both sides, whose own permissions, owner and
	/// timestamp are carried over even when nothing inside them changed.
	///
	/// A mode change on a directory is exactly the kind of thing a rollback is
	/// for — postgres refuses to start a data directory that is not 0700 or
	/// 0750 — and it leaves no trace in the entries the walk otherwise collects.
	pub retouch: Vec<PathBuf>,
	/// Entries to copy from the capture.
	pub copy: Vec<PathBuf>,
	/// Entries to remove from the live tree, children before parents.
	pub remove: Vec<Removal>,
	/// Bytes the entries in `copy` occupy in the capture.
	pub copy_bytes: u64,
	/// Bytes the entries in `remove` occupy in the live tree.
	pub remove_bytes: u64,
	/// Bytes the entries in `copy` currently occupy in the live tree, where they
	/// exist there at all. The difference between this and `copy_bytes` is the
	/// net growth, which is what filesystem free space has to absorb.
	pub displaced_bytes: u64,
}

impl Delta {
	/// Whether there is anything to write or remove.
	pub fn has_work(&self) -> bool {
		!self.mkdir.is_empty() || !self.copy.is_empty() || !self.remove.is_empty()
	}

	/// Net bytes the live tree grows by, which is what its filesystem has to have
	/// free. Saturates at zero: a delta that frees more than it adds needs none.
	pub fn net_growth(&self) -> u64 {
		self.copy_bytes
			.saturating_sub(self.displaced_bytes)
			.saturating_sub(self.remove_bytes)
	}

	/// Total bytes written into the live tree, which is what a copy-on-write
	/// store has to absorb: every block written over a block the capture still
	/// references is copied aside before it is overwritten.
	pub fn written_bytes(&self) -> u64 {
		self.copy_bytes
	}
}

/// Compare `capture` against `dest`, producing everything the restore must do.
///
/// `skip` names paths (relative to the roots) the walk must not look at, which
/// is how the interlock keeps its own files out of the sync so they can be
/// written back last.
pub async fn compare(capture: &Path, dest: &Path, skip: &[PathBuf], decide: Decide) -> Result<Delta> {
	let capture = capture.to_path_buf();
	let dest = dest.to_path_buf();
	let skip = skip.to_vec();
	tokio::task::spawn_blocking(move || {
		let mut walk = Walk {
			capture: &capture,
			dest: &dest,
			skip: &skip,
			// Reused across every comparison rather than allocated per file: on a
			// tree with many same-size entries the allocation would dominate. Not
			// allocated at all where a basis answers, since nothing is then read.
			ours: compare_buffer(&decide),
			theirs: compare_buffer(&decide),
			decide,
			read: 0,
			delta: Delta::default(),
		};
		walk.descend(Path::new(""))?;
		if walk.read > 0 {
			debug!(bytes = walk.read, "compared the same-size entries by content");
		}
		Ok(walk.delta)
	})
	.await
	.into_diagnostic()
	.wrap_err("joining the capture comparison")?
}

fn compare_buffer(decide: &Decide) -> Vec<u8> {
	match decide {
		Decide::Compare => vec![0u8; COMPARE_CHUNK],
		Decide::Named(_) => Vec::new(),
	}
}

struct Walk<'a> {
	capture: &'a Path,
	dest: &'a Path,
	skip: &'a [PathBuf],
	decide: Decide,
	ours: Vec<u8>,
	theirs: Vec<u8>,
	read: u64,
	delta: Delta,
}

/// One side's entries in a directory, by name, with the metadata the comparison
/// needs. An unreadable directory is an error rather than an omission: silently
/// treating it as empty would delete everything under it.
fn read_side(dir: &Path) -> Result<Option<BTreeMap<std::ffi::OsString, std::fs::Metadata>>> {
	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
		Err(err) => {
			return Err(err)
				.into_diagnostic()
				.wrap_err_with(|| format!("reading {}", dir.display()));
		}
	};
	let mut out = BTreeMap::new();
	for entry in entries {
		let entry = entry
			.into_diagnostic()
			.wrap_err_with(|| format!("reading an entry of {}", dir.display()))?;
		// `DirEntry::metadata` does not follow symlinks — a tablespace link in
		// `pg_tblspc` is part of the captured state, and following it would compare
		// whatever it points at — and on Windows it is answered from the directory
		// listing already read, with no further syscall.
		let meta = entry
			.metadata()
			.into_diagnostic()
			.wrap_err_with(|| format!("stating {}", entry.path().display()))?;
		out.insert(entry.file_name(), meta);
	}
	Ok(Some(out))
}

impl Walk<'_> {
	fn skipped(&self, rel: &Path) -> bool {
		self.skip.iter().any(|s| s == rel)
	}

	/// Compare one directory present on both sides.
	fn descend(&mut self, rel: &Path) -> Result<()> {
		let Some(from) = read_side(&self.capture.join(rel))? else {
			// The capture has no such directory. Only directories present on both
			// sides are descended into, so this is one that vanished between the two
			// reads.
			return Ok(());
		};
		let into = read_side(&self.dest.join(rel))?.unwrap_or_default();

		for (name, capture_meta) in &from {
			let child = rel.join(name);
			if self.skipped(&child) {
				continue;
			}
			let live_meta = into.get(name);
			match classify(capture_meta, live_meta) {
				Some(Change::Copy) if capture_meta.is_dir() => {
					// The live tree has no directory here: either nothing, or something
					// of another kind, which goes.
					self.delta.mkdir.push(child.clone());
					if let Some(live) = live_meta {
						self.delta.remove_bytes = self.delta.remove_bytes.saturating_add(live.len());
						self.delta.remove.push(Removal {
							rel: child.clone(),
							is_dir: false,
						});
					}
					// Deliberately not `descend`: the live side is not a directory, so
					// reading it would either fail outright (a regular file gives
					// `NotADirectory`, which is not `NotFound`) or follow a symlink out
					// of the tree and schedule everything under its *target* for
					// removal. Everything under a captured directory the live tree does
					// not have is a copy regardless.
					self.copy_subtree(&child)?;
				}
				Some(Change::Copy) => self.schedule_copy(child, capture_meta, live_meta),
				Some(Change::SameSize { mtime_same, len }) => {
					if self.diverged(&child, mtime_same) {
						self.delta.copy_bytes = self.delta.copy_bytes.saturating_add(len);
						// Replacing an entry with one of the same size adds nothing to the
						// tree, but is still a full write to the copy-on-write store.
						self.delta.displaced_bytes = self.delta.displaced_bytes.saturating_add(len);
						self.delta.copy.push(child);
					}
				}
				None => {
					self.delta.retouch.push(child.clone());
					self.descend(&child)?;
				}
			}
		}

		// Anything the live tree has that the capture does not was written after
		// the freeze, so it is not part of the state being rolled back to.
		for (name, live_meta) in &into {
			if from.contains_key(name) {
				continue;
			}
			let child = rel.join(name);
			if self.skipped(&child) {
				continue;
			}
			// `is_dir` on symlink metadata, so a symlink to a directory is unlinked
			// rather than descended into and emptied.
			if live_meta.is_dir() {
				// A skipped entry underneath keeps its directory: removing a parent
				// whose child was deliberately left would fail on a non-empty
				// directory, and taking the child with it would undo the very skip.
				if self.remove_subtree(&child)? {
					continue;
				}
			}
			self.delta.remove_bytes = self.delta.remove_bytes.saturating_add(live_meta.len());
			self.delta.remove.push(Removal {
				rel: child,
				is_dir: live_meta.is_dir(),
			});
		}

		Ok(())
	}

	fn schedule_copy(
		&mut self,
		child: PathBuf,
		capture_meta: &std::fs::Metadata,
		live_meta: Option<&std::fs::Metadata>,
	) {
		self.delta.copy_bytes = self.delta.copy_bytes.saturating_add(capture_meta.len());
		if let Some(live) = live_meta {
			// Replacing an entry in place: only the growth is new allocation.
			self.delta.displaced_bytes = self.delta.displaced_bytes.saturating_add(live.len());
		}
		self.delta.copy.push(child);
	}

	/// Everything under a captured directory the live tree has nothing usable at,
	/// parents before children, so it all comes back.
	fn copy_subtree(&mut self, rel: &Path) -> Result<()> {
		let Some(entries) = read_side(&self.capture.join(rel))? else {
			return Ok(());
		};
		for (name, meta) in &entries {
			let child = rel.join(name);
			if self.skipped(&child) {
				continue;
			}
			if meta.is_dir() {
				self.delta.mkdir.push(child.clone());
				self.copy_subtree(&child)?;
			} else {
				self.schedule_copy(child, meta, None);
			}
		}
		Ok(())
	}

	/// Every entry under a directory that is going, children before the directory
	/// itself, so the removal is a plain sequence of unlinks and rmdirs.
	///
	/// Returns whether anything under it was kept, which is what tells the caller
	/// the directory itself has to stay. The skip list is honoured here as it is
	/// everywhere else the walk descends: an interlock file that fell under a
	/// directory the capture lacks would otherwise be removed, undoing the skip
	/// that protects it.
	fn remove_subtree(&mut self, rel: &Path) -> Result<bool> {
		let Some(entries) = read_side(&self.dest.join(rel))? else {
			return Ok(false);
		};
		let mut kept = false;
		for (name, meta) in &entries {
			let child = rel.join(name);
			if self.skipped(&child) {
				kept = true;
				continue;
			}
			if meta.is_dir() && self.remove_subtree(&child)? {
				kept = true;
				continue;
			}
			self.delta.remove_bytes = self.delta.remove_bytes.saturating_add(meta.len());
			self.delta.remove.push(Removal {
				rel: child,
				is_dir: meta.is_dir(),
			});
		}
		Ok(kept)
	}

	/// Whether a same-size entry's contents differ from the capture's.
	fn diverged(&mut self, rel: &Path, mtime_same: bool) -> bool {
		match &self.decide {
			Decide::Named(paths) => paths.contains(rel),
			// A matching modification time is taken as matching contents. This is the
			// degraded rule, and the reason the mode reports which of the two it got.
			Decide::Compare if mtime_same => false,
			Decide::Compare => {
				let read = same_contents(
					&self.capture.join(rel),
					&self.dest.join(rel),
					&mut self.ours,
					&mut self.theirs,
				);
				self.read = self.read.saturating_add(read.bytes);
				!read.same
			}
		}
	}
}

/// What the entry in the capture should become in the live tree.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
	/// Copy it: the live tree has nothing there, something of another kind, or
	/// something of a different size.
	Copy,
	/// Present in both as the same kind of thing at the same size. Whether the
	/// contents diverged is not knowable from metadata.
	SameSize { mtime_same: bool, len: u64 },
}

/// What to do about one capture entry given the live tree's, if any.
///
/// `None` means both sides are directories, which the caller descends into
/// rather than acting on.
fn classify(capture: &std::fs::Metadata, live: Option<&std::fs::Metadata>) -> Option<Change> {
	let Some(live) = live else {
		return Some(Change::Copy);
	};
	if capture.file_type() != live.file_type() {
		return Some(Change::Copy);
	}
	if capture.is_dir() {
		return None;
	}
	// A symlink is small and comparing its target costs a readlink, so it is
	// simply rewritten; its size is the target's length, not an allocation.
	if capture.is_symlink() || capture.len() != live.len() {
		return Some(Change::Copy);
	}
	Some(Change::SameSize {
		mtime_same: same_mtime(capture, live),
		len: capture.len(),
	})
}

/// Whether two entries carry the same modification time. An unreadable time on
/// either side is treated as a difference, so the pair is examined rather than
/// assumed identical.
fn same_mtime(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
	match (a.modified(), b.modified()) {
		(Ok(a), Ok(b)) => a == b,
		_ => false,
	}
}

/// The outcome of reading two files against each other.
struct Comparison {
	same: bool,
	/// Bytes read from both sides together, for reporting what the fallback cost.
	bytes: u64,
}

/// Whether two files hold the same bytes, reading them in lockstep and stopping
/// at the first difference.
///
/// Hashing each side whole would cost a full read of both however early they
/// diverge, and the answer wanted here is only "same or not". Either side being
/// unreadable counts as a difference, so the capture's copy is laid down rather
/// than a doubtful one kept.
fn same_contents(a: &Path, b: &Path, ours: &mut [u8], theirs: &mut [u8]) -> Comparison {
	use std::io::Read as _;

	/// Read until the buffer is full or the file ends, so a short read does not
	/// misalign the two sides against each other.
	fn fill(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
		let mut filled = 0;
		while filled < buf.len() {
			match file.read(&mut buf[filled..]) {
				Ok(0) => break,
				Ok(n) => filled += n,
				Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
				Err(err) => return Err(err),
			}
		}
		Ok(filled)
	}

	let (Ok(mut a), Ok(mut b)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
		return Comparison { same: false, bytes: 0 };
	};
	let mut bytes = 0u64;
	loop {
		let (Ok(mine), Ok(yours)) = (fill(&mut a, ours), fill(&mut b, theirs)) else {
			return Comparison { same: false, bytes };
		};
		bytes = bytes.saturating_add((mine + yours) as u64);
		if mine != yours || ours[..mine] != theirs[..yours] {
			return Comparison { same: false, bytes };
		}
		if mine == 0 {
			return Comparison { same: true, bytes };
		}
	}
}

/// Lay the delta down: remove what the capture does not have, make the
/// directories it does, and copy its entries over the live tree's.
///
/// Removals run first, so a path that changed kind has its old entry out of the
/// way before the new one is written, and so a tree that grew since the freeze
/// gives its space back before the copy asks for any.
pub async fn apply(capture: &Path, dest: &Path, delta: &Delta) -> Result<()> {
	// Children were pushed before their parents, so removing in order unlinks a
	// directory's contents before the directory. The whole pass runs on one
	// blocking thread: a delta with many post-freeze files is otherwise that many
	// thread-pool round trips for what is a sequence of unlinks.
	let removals: Vec<(PathBuf, bool)> = delta
		.remove
		.iter()
		.map(|entry| (dest.join(&entry.rel), entry.is_dir))
		.collect();
	tokio::task::spawn_blocking(move || {
		for (path, is_dir) in removals {
			remove_entry(&path, is_dir)
				.into_diagnostic()
				.wrap_err_with(|| format!("removing {}", path.display()))?;
		}
		Ok::<_, miette::Report>(())
	})
	.await
	.into_diagnostic()
	.wrap_err("joining the removal pass")??;

	// Parents were pushed before their children, so each one only needs its own
	// component made — `create_dir_all` would re-stat every ancestor of every
	// directory. One blocking task for the pass, as the removals are: a restore
	// onto an absent destination schedules the capture's whole directory tree.
	let directories: Vec<PathBuf> = delta.mkdir.iter().map(|rel| dest.join(rel)).collect();
	tokio::task::spawn_blocking(move || {
		for path in directories {
			match std::fs::create_dir(&path) {
				Ok(()) => {}
				Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
				// A parent can be missing when the destination itself is not there.
				Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
					std::fs::create_dir_all(&path)
						.into_diagnostic()
						.wrap_err_with(|| format!("creating {}", path.display()))?;
				}
				Err(err) => {
					return Err(err)
						.into_diagnostic()
						.wrap_err_with(|| format!("creating {}", path.display()));
				}
			}
		}
		Ok::<_, miette::Report>(())
	})
	.await
	.into_diagnostic()
	.wrap_err("joining the directory pass")??;

	let total = delta.copy.len();
	let mut done = 0usize;
	let mut copied = 0u64;
	let mut last_parent: Option<&Path> = None;
	let mut last_report = std::time::Instant::now();
	for rel in &delta.copy {
		// The mkdir phase has already made every directory the capture carries, so
		// a parent only needs creating when the copy list reaches outside the one
		// before it — and consecutive entries almost always share a parent.
		let parent = rel.parent();
		let fresh_parent = parent.is_some() && parent != last_parent;
		copied =
			copied.saturating_add(copy_entry(&capture.join(rel), &dest.join(rel), fresh_parent).await?);
		if fresh_parent {
			last_parent = parent;
		}
		done += 1;
		// A large sync is otherwise a silent wait, and this one runs against a
		// cluster that is down.
		if last_report.elapsed() >= std::time::Duration::from_secs(30) {
			info!(done, total, bytes = copied, "copying the divergence into place");
			last_report = std::time::Instant::now();
		}
	}

	// A directory's own timestamps change as its contents are written, so they
	// are set after the copies rather than when the directory is made. Deepest
	// first, so a parent's timestamp is not disturbed by a child being touched
	// after it — and covering the directories that were already there too, whose
	// mode or owner can have drifted since the capture without anything inside
	// them changing.
	let directories: Vec<(PathBuf, PathBuf)> = delta
		.mkdir
		.iter()
		.chain(&delta.retouch)
		.rev()
		.map(|rel| (capture.join(rel), dest.join(rel)))
		.collect();
	tokio::task::spawn_blocking(move || {
		for (from, to) in directories {
			if let Ok(meta) = from.symlink_metadata() {
				carry_metadata(&meta, &to);
			}
		}
	})
	.await
	.into_diagnostic()
	.wrap_err("joining the directory metadata pass")?;

	Ok(())
}

/// Remove one entry, given what the walk already established it is.
///
/// On Windows a directory symlink or junction — a relocated `pg_wal`, or the
/// junctions this codebase makes to expose a shadow copy — is removed with
/// `RemoveDirectoryW` rather than `DeleteFileW`, and the metadata does not say
/// which kind of link it is. Getting it wrong fails with access denied partway
/// through, on the platform the VSS path exists for.
pub(super) fn remove_entry(path: &Path, is_dir: bool) -> std::io::Result<()> {
	if is_dir {
		return std::fs::remove_dir(path);
	}
	#[cfg(windows)]
	{
		if path.symlink_metadata().is_ok_and(|meta| meta.is_symlink()) {
			return std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path));
		}
	}
	match std::fs::remove_file(path) {
		// The walk saw it; something else removing it first is not a failure.
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
		other => other,
	}
}

/// Copy one entry from the capture over the live tree's, returning the bytes
/// written.
///
/// The whole operation — stat, replace whatever is there, copy, carry the
/// permissions and timestamps over — happens on one blocking thread. Splitting
/// it costs a scheduling round trip and a repeated stat per entry, which on a
/// delta of many thousands of small files outweighs the bytes moved.
pub async fn copy_entry(from: &Path, to: &Path, make_parent: bool) -> Result<u64> {
	let from = from.to_path_buf();
	let to = to.to_path_buf();
	tokio::task::spawn_blocking(move || copy_entry_blocking(&from, &to, make_parent))
		.await
		.into_diagnostic()
		.wrap_err("joining the entry copy")?
}

fn copy_entry_blocking(from: &Path, to: &Path, make_parent: bool) -> Result<u64> {
	let meta = from
		.symlink_metadata()
		.into_diagnostic()
		.wrap_err_with(|| format!("stating {}", from.display()))?;

	if make_parent
		&& let Some(parent) = to.parent()
	{
		std::fs::create_dir_all(parent)
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", parent.display()))?;
	}

	// Whatever is there now may be the wrong kind, or a symlink that a plain
	// write would follow out of the tree.
	match to.symlink_metadata() {
		Ok(existing) if existing.is_dir() => {
			std::fs::remove_dir_all(to)
				.into_diagnostic()
				.wrap_err_with(|| format!("removing {} to replace it", to.display()))?;
		}
		Ok(existing) if existing.is_symlink() || meta.is_symlink() => {
			// `is_dir` is false for a link, so this goes through the same removal
			// the delta's own does, which knows a Windows directory link needs
			// `RemoveDirectoryW`.
			remove_entry(to, false)
				.into_diagnostic()
				.wrap_err_with(|| format!("removing {} to replace it", to.display()))?;
		}
		_ => {}
	}

	if meta.is_symlink() {
		let target = std::fs::read_link(from)
			.into_diagnostic()
			.wrap_err_with(|| format!("reading the link {}", from.display()))?;
		symlink(&target, to)?;
		return Ok(0);
	}
	if meta.is_dir() {
		std::fs::create_dir_all(to)
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", to.display()))?;
		carry_metadata(&meta, to);
		return Ok(0);
	}
	if !meta.is_file() {
		// Sockets and the like are live-process artefacts, not captured state.
		debug!("skipping {} ({:?})", from.display(), meta.file_type());
		return Ok(0);
	}

	std::fs::copy(from, to)
		.into_diagnostic()
		.wrap_err_with(|| format!("copying {} to {}", from.display(), to.display()))?;
	carry_metadata(&meta, to);
	Ok(meta.len())
}

#[cfg(unix)]
fn symlink(target: &Path, at: &Path) -> Result<()> {
	std::os::unix::fs::symlink(target, at)
		.into_diagnostic()
		.wrap_err_with(|| format!("linking {} -> {}", at.display(), target.display()))
}

#[cfg(windows)]
fn symlink(target: &Path, at: &Path) -> Result<()> {
	// The capture's own entry says which kind it is; a directory link made as a
	// file link resolves to nothing.
	let result = if target.is_dir() {
		std::os::windows::fs::symlink_dir(target, at)
	} else {
		std::os::windows::fs::symlink_file(target, at)
	};
	result
		.into_diagnostic()
		.wrap_err_with(|| format!("linking {} -> {}", at.display(), target.display()))
}

/// Carry the capture's permissions, ownership, and modification time onto the
/// entry written from it, so the restored tree is the captured tree rather than
/// one wearing the restoring process's umask and clock.
///
/// Takes the capture entry's metadata rather than re-reading it: every caller
/// has just stat'd the same path.
///
/// Best-effort on each attribute: postgres is started as its service account
/// afterwards and the restore fixes ownership across the whole tree at that
/// point, so one failure here is not worth abandoning a restore over.
fn carry_metadata(meta: &std::fs::Metadata, to: &Path) {
	// A symlink's own metadata is not settable through these APIs, and the target
	// it points at must not be touched in its place.
	if meta.is_symlink() {
		return;
	}
	#[cfg(unix)]
	{
		use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
		// Ownership first: Linux clears the setuid and setgid bits on `chown`, so
		// setting the mode before it would silently drop them — on a whole-install
		// restore that is the captured binaries, and on any tree it is setgid
		// directories.
		if let Err(err) = std::os::unix::fs::chown(to, Some(meta.uid()), Some(meta.gid())) {
			debug!("could not set the owner of {}: {err}", to.display());
		}
		if let Err(err) =
			std::fs::set_permissions(to, std::fs::Permissions::from_mode(meta.permissions().mode()))
		{
			debug!("could not set the mode of {}: {err}", to.display());
		}
	}
	#[cfg(not(unix))]
	{
		if let Err(err) = std::fs::set_permissions(to, meta.permissions()) {
			debug!("could not set the permissions of {}: {err}", to.display());
		}
	}
	if let Ok(mtime) = meta.modified()
		&& let Err(err) = filetime::set_file_mtime(to, filetime::FileTime::from(mtime))
	{
		debug!("could not set the mtime of {}: {err}", to.display());
	}
}

/// Check that the capture is readable enough to restore from before anything is
/// written, so an unreadable capture is refused rather than discovered halfway
/// through overwriting the tree it was meant to replace.
pub async fn ensure_readable(capture: &Path) -> Result<()> {
	match read_side(capture) {
		Ok(Some(entries)) if !entries.is_empty() => Ok(()),
		Ok(Some(_)) => bail!(
			"the capture at {} is an empty directory, so restoring from it would \
			 erase the cluster rather than roll it back",
			capture.display()
		),
		Ok(None) => bail!("the capture at {} is not there", capture.display()),
		Err(err) => Err(err),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn write(root: &Path, rel: &str, contents: &str) {
		let path = root.join(rel);
		std::fs::create_dir_all(path.parent().unwrap()).unwrap();
		std::fs::write(path, contents).unwrap();
	}

	/// The two trees and the delta between them, compared by content.
	async fn delta_of(build: impl FnOnce(&Path, &Path)) -> (tempfile::TempDir, PathBuf, PathBuf, Delta) {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		std::fs::create_dir_all(&capture).unwrap();
		std::fs::create_dir_all(&dest).unwrap();
		build(&capture, &dest);
		let delta = compare(&capture, &dest, &[], Decide::Compare).await.unwrap();
		(tmp, capture, dest, delta)
	}

	/// The paths a delta removes, in order.
	fn removed(delta: &Delta) -> Vec<&Path> {
		delta.remove.iter().map(|entry| entry.rel.as_path()).collect()
	}

	/// Compare and then lay down, which is what a restore does.
	async fn roll_back(capture: &Path, dest: &Path, skip: &[PathBuf]) {
		let delta = compare(capture, dest, skip, Decide::Compare).await.unwrap();
		apply(capture, dest, &delta).await.unwrap();
	}

	#[tokio::test]
	async fn identical_trees_have_nothing_to_do() {
		let (_tmp, _c, _d, delta) = delta_of(|capture, dest| {
			write(capture, "PG_VERSION", "16");
			write(capture, "base/1/2345", "page");
			write(dest, "PG_VERSION", "16");
			write(dest, "base/1/2345", "page");
		})
		.await;
		assert!(!delta.has_work());
	}

	#[tokio::test]
	async fn a_file_written_since_the_freeze_is_removed() {
		let (_tmp, _c, _d, delta) = delta_of(|capture, dest| {
			write(capture, "PG_VERSION", "16");
			write(dest, "PG_VERSION", "16");
			write(dest, "base/1/9999", "written after the capture");
		})
		.await;
		let removes = removed(&delta);
		assert!(removes.contains(&Path::new("base/1/9999")));
		assert!(removes.contains(&Path::new("base")));
		// Children are removed before their parents.
		let file = removes.iter().position(|p| p.ends_with("9999")).unwrap();
		let dir = removes.iter().position(|p| *p == Path::new("base")).unwrap();
		assert!(file < dir, "children must be removed before their parents");
	}

	#[tokio::test]
	async fn a_file_deleted_since_the_freeze_comes_back() {
		let (_tmp, _c, _d, delta) = delta_of(|capture, dest| {
			write(capture, "base/1/2345", "page");
			write(dest, "PG_VERSION", "16");
		})
		.await;
		assert!(delta.copy.contains(&PathBuf::from("base/1/2345")));
		assert!(delta.mkdir.contains(&PathBuf::from("base")));
		assert!(removed(&delta).contains(&Path::new("PG_VERSION")));
	}

	#[tokio::test]
	async fn a_size_change_is_decided_by_the_walk_alone() {
		let (_tmp, _c, _d, delta) = delta_of(|capture, dest| {
			write(capture, "base/1/2345", "grown since the freeze");
			write(dest, "base/1/2345", "short");
		})
		.await;
		assert_eq!(delta.copy, vec![PathBuf::from("base/1/2345")]);
		// Replacing in place: only the growth is new allocation.
		assert_eq!(delta.displaced_bytes, "short".len() as u64);
		assert_eq!(delta.copy_bytes, "grown since the freeze".len() as u64);
	}

	#[tokio::test]
	async fn skipped_paths_are_left_out_of_both_sides() {
		// Differing sizes, so the walk alone would decide to copy it.
		let (_tmp, _c, _d, delta) = delta_of(|capture, dest| {
			write(capture, "PG_VERSION", "16");
			write(dest, "PG_VERSION", "9.6");
		})
		.await;
		assert_eq!(delta.copy, vec![PathBuf::from("PG_VERSION")]);

		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "PG_VERSION", "16");
		write(&dest, "PG_VERSION", "9.6");
		let delta = compare(&capture, &dest, &[PathBuf::from("PG_VERSION")], Decide::Compare)
			.await
			.unwrap();
		assert!(!delta.has_work(), "the interlock's file must not be synced");
	}

	#[tokio::test]
	async fn a_skipped_path_under_a_replaced_directory_is_still_skipped() {
		// The copy-subtree path has its own skip check, so an interlock file inside
		// a directory the live tree lacks is not scheduled behind its back.
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "data/PG_VERSION", "16");
		write(&capture, "data/base/1", "page");
		std::fs::create_dir_all(&dest).unwrap();

		let delta = compare(&capture, &dest, &[PathBuf::from("data/PG_VERSION")], Decide::Compare)
			.await
			.unwrap();
		assert!(delta.copy.contains(&PathBuf::from("data/base/1")));
		assert!(!delta.copy.contains(&PathBuf::from("data/PG_VERSION")));
	}

	/// Every other walker honours the skip list, so this one must too: an
	/// interlock file under a directory the capture lacks would otherwise be
	/// removed, undoing the skip that protects it — and the parent's `rmdir`
	/// would then fail on a directory that is not empty after all.
	#[tokio::test]
	async fn a_skipped_file_keeps_its_directory_from_being_removed() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "PG_VERSION", "16");
		write(&dest, "PG_VERSION", "16");
		write(&dest, "gone/keep-me", "protected");
		write(&dest, "gone/take-me", "post-freeze");

		let skip = vec![PathBuf::from("gone/keep-me")];
		let delta = compare(&capture, &dest, &skip, Decide::Compare).await.unwrap();

		let removes = removed(&delta);
		assert!(removes.contains(&Path::new("gone/take-me")));
		assert!(!removes.contains(&Path::new("gone/keep-me")), "the skip must hold");
		assert!(
			!removes.contains(&Path::new("gone")),
			"a directory with something kept in it cannot be removed"
		);

		// And it applies cleanly, which it would not if the non-empty parent were
		// scheduled for removal.
		apply(&capture, &dest, &delta).await.unwrap();
		assert!(dest.join("gone/keep-me").exists());
		assert!(!dest.join("gone/take-me").exists());
	}

	#[tokio::test]
	async fn a_directory_with_nothing_kept_in_it_still_goes() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "PG_VERSION", "16");
		write(&dest, "PG_VERSION", "16");
		write(&dest, "gone/take-me", "post-freeze");

		roll_back(&capture, &dest, &[PathBuf::from("elsewhere")]).await;
		assert!(!dest.join("gone").exists());
	}

	/// A directory that exists on both sides carries no entry of its own in the
	/// delta, so a mode changed since the capture would otherwise never roll back
	/// — and postgres refuses to start a data directory that is not 0700 or 0750.
	#[cfg(unix)]
	#[tokio::test]
	async fn a_directory_whose_mode_drifted_is_put_back() {
		use std::os::unix::fs::PermissionsExt as _;

		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "data/PG_VERSION", "16");
		write(&dest, "data/PG_VERSION", "16");
		std::fs::set_permissions(capture.join("data"), std::fs::Permissions::from_mode(0o700))
			.unwrap();
		std::fs::set_permissions(dest.join("data"), std::fs::Permissions::from_mode(0o777)).unwrap();

		roll_back(&capture, &dest, &[]).await;

		let mode = std::fs::metadata(dest.join("data")).unwrap().permissions().mode();
		assert_eq!(mode & 0o777, 0o700, "got {mode:o}");
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn ownership_is_set_before_the_mode_so_setgid_survives() {
		// Linux clears the setuid and setgid bits on `chown`, so setting the mode
		// first would drop them from the restored tree.
		use std::os::unix::fs::PermissionsExt as _;

		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "setgid-dir/f", "x");
		std::fs::set_permissions(capture.join("setgid-dir"), std::fs::Permissions::from_mode(0o2755))
			.unwrap();
		std::fs::create_dir_all(&dest).unwrap();

		roll_back(&capture, &dest, &[]).await;

		let mode = std::fs::metadata(dest.join("setgid-dir")).unwrap().permissions().mode();
		assert_eq!(mode & 0o7777, 0o2755, "the setgid bit must come back, got {mode:o}");
	}

	#[tokio::test]
	async fn same_size_different_mtime_but_same_content_is_not_copied() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "f", "sixteen bytes!!!");
		write(&dest, "f", "sixteen bytes!!!");
		filetime::set_file_mtime(dest.join("f"), filetime::FileTime::from_unix_time(1, 0)).unwrap();

		let delta = compare(&capture, &dest, &[], Decide::Compare).await.unwrap();
		assert!(!delta.has_work(), "identical contents must not be copied");
	}

	#[tokio::test]
	async fn same_size_different_mtime_and_different_content_is_copied() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "f", "sixteen bytes!!!");
		write(&dest, "f", "SIXTEEN BYTES!!!");
		filetime::set_file_mtime(dest.join("f"), filetime::FileTime::from_unix_time(1, 0)).unwrap();

		let delta = compare(&capture, &dest, &[], Decide::Compare).await.unwrap();
		assert_eq!(delta.copy, vec![PathBuf::from("f")]);
	}

	#[tokio::test]
	async fn identical_mtime_and_size_is_skipped_without_reading() {
		// The degraded rule: the contents differ, but size and mtime agree, so the
		// fallback keeps the live file. This is the gap a basis closes.
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "f", "sixteen bytes!!!");
		write(&dest, "f", "SIXTEEN BYTES!!!");
		let when = filetime::FileTime::from_unix_time(1, 0);
		filetime::set_file_mtime(capture.join("f"), when).unwrap();
		filetime::set_file_mtime(dest.join("f"), when).unwrap();

		let delta = compare(&capture, &dest, &[], Decide::Compare).await.unwrap();
		assert!(!delta.has_work());
	}

	#[tokio::test]
	async fn a_named_basis_overrides_a_matching_modification_time() {
		// The point of a basis: it closes the gap the fallback leaves, so an entry
		// it names is copied even though size and mtime both match.
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "f", "sixteen bytes!!!");
		write(&dest, "f", "SIXTEEN BYTES!!!");
		let when = filetime::FileTime::from_unix_time(1, 0);
		filetime::set_file_mtime(capture.join("f"), when).unwrap();
		filetime::set_file_mtime(dest.join("f"), when).unwrap();

		let named = Decide::Named(BTreeSet::from([PathBuf::from("f")]));
		let delta = compare(&capture, &dest, &[], named).await.unwrap();
		assert_eq!(delta.copy, vec![PathBuf::from("f")]);
	}

	#[tokio::test]
	async fn a_named_basis_leaves_alone_what_it_does_not_name() {
		// Authoritative the other way too: an entry outside the set is not read,
		// whatever its timestamp says.
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "f", "sixteen bytes!!!");
		write(&dest, "f", "SIXTEEN BYTES!!!");
		filetime::set_file_mtime(dest.join("f"), filetime::FileTime::from_unix_time(1, 0)).unwrap();

		let delta = compare(&capture, &dest, &[], Decide::Named(BTreeSet::new()))
			.await
			.unwrap();
		assert!(!delta.has_work());
	}

	#[tokio::test]
	async fn applying_makes_the_live_tree_match_the_capture() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "PG_VERSION", "16");
		write(&capture, "base/1/2345", "the captured page");
		write(&capture, "global/pg_control", "control");
		write(&dest, "PG_VERSION", "16");
		write(&dest, "base/1/2345", "a page written after the freeze");
		write(&dest, "base/1/9999", "a relation created after the freeze");

		roll_back(&capture, &dest, &[]).await;

		assert_eq!(std::fs::read_to_string(dest.join("base/1/2345")).unwrap(), "the captured page");
		assert_eq!(std::fs::read_to_string(dest.join("global/pg_control")).unwrap(), "control");
		assert!(!dest.join("base/1/9999").exists(), "post-freeze relation must go");
	}

	#[tokio::test]
	async fn applying_twice_converges() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "base/1/2345", "the captured page");
		write(&dest, "base/1/2345", "diverged");
		write(&dest, "stray", "written after the freeze");

		roll_back(&capture, &dest, &[]).await;
		roll_back(&capture, &dest, &[]).await;

		// The second pass has nothing left to do, which is what makes a failed
		// restore resumable rather than restartable.
		let delta = compare(&capture, &dest, &[], Decide::Compare).await.unwrap();
		assert!(!delta.has_work());
		assert_eq!(std::fs::read_to_string(dest.join("base/1/2345")).unwrap(), "the captured page");
		assert!(!dest.join("stray").exists());
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn a_symlink_is_restored_as_a_link_not_its_target() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		std::fs::create_dir_all(capture.join("pg_tblspc")).unwrap();
		std::fs::create_dir_all(&dest).unwrap();
		std::os::unix::fs::symlink("/mnt/fast/tblspc", capture.join("pg_tblspc/16400")).unwrap();

		roll_back(&capture, &dest, &[]).await;

		let link = dest.join("pg_tblspc/16400");
		assert!(link.symlink_metadata().unwrap().is_symlink());
		assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("/mnt/fast/tblspc"));
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn a_captured_directory_over_a_live_symlink_never_reaches_its_target() {
		// A relocated `pg_wal` is an ordinary layout. Descending into the live side
		// here would follow the link and schedule everything under its target for
		// removal, destroying data outside the tree being restored.
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		let outside = tmp.path().join("outside");
		std::fs::create_dir_all(capture.join("pg_wal")).unwrap();
		std::fs::write(capture.join("pg_wal/000001"), "captured wal").unwrap();
		std::fs::create_dir_all(&dest).unwrap();
		std::fs::create_dir_all(&outside).unwrap();
		std::fs::write(outside.join("important"), "data outside the tree").unwrap();
		std::os::unix::fs::symlink(&outside, dest.join("pg_wal")).unwrap();

		let delta = compare(&capture, &dest, &[], Decide::Compare).await.unwrap();
		assert_eq!(
			removed(&delta),
			vec![Path::new("pg_wal")],
			"only the link itself goes, never anything through it"
		);

		apply(&capture, &dest, &delta).await.unwrap();
		assert!(
			outside.join("important").exists(),
			"data outside the restored tree must survive"
		);
		assert_eq!(
			std::fs::read_to_string(dest.join("pg_wal/000001")).unwrap(),
			"captured wal"
		);
	}

	#[tokio::test]
	async fn a_captured_directory_over_a_live_file_does_not_abort_the_comparison() {
		// Reading the live side as a directory fails with `NotADirectory`, which is
		// not `NotFound`, so it would surface as an error and abort the restore
		// before anything was written.
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		std::fs::create_dir_all(capture.join("pg_notify")).unwrap();
		std::fs::write(capture.join("pg_notify/0000"), "captured").unwrap();
		std::fs::create_dir_all(&dest).unwrap();
		std::fs::write(dest.join("pg_notify"), "a file where a directory belongs").unwrap();

		roll_back(&capture, &dest, &[]).await;

		assert!(dest.join("pg_notify").is_dir());
		assert_eq!(std::fs::read_to_string(dest.join("pg_notify/0000")).unwrap(), "captured");
	}

	#[tokio::test]
	async fn an_empty_captured_directory_comes_back() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		// `pg_notify` and friends are empty in a freshly-checkpointed cluster, and
		// postgres expects them to exist.
		std::fs::create_dir_all(capture.join("pg_notify")).unwrap();
		std::fs::create_dir_all(&dest).unwrap();

		roll_back(&capture, &dest, &[]).await;
		assert!(dest.join("pg_notify").is_dir());
	}

	#[tokio::test]
	async fn an_entry_that_changed_kind_is_replaced_rather_than_written_through() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "thing", "a file in the capture");
		std::fs::create_dir_all(dest.join("thing")).unwrap();
		std::fs::write(dest.join("thing/inside"), "should not survive").unwrap();

		roll_back(&capture, &dest, &[]).await;

		assert!(dest.join("thing").is_file());
		assert_eq!(
			std::fs::read_to_string(dest.join("thing")).unwrap(),
			"a file in the capture"
		);
	}

	#[test]
	fn content_comparison_stops_at_the_first_difference() {
		// Two large files differing in their first bytes: the comparison must not
		// read either of them whole.
		let tmp = tempfile::tempdir().unwrap();
		let a = tmp.path().join("a");
		let b = tmp.path().join("b");
		let ours = vec![b'x'; 4 * COMPARE_CHUNK];
		let mut theirs = ours.clone();
		theirs[0] = b'y';
		std::fs::write(&a, &ours).unwrap();
		std::fs::write(&b, &theirs).unwrap();

		let mut buf_a = vec![0u8; COMPARE_CHUNK];
		let mut buf_b = vec![0u8; COMPARE_CHUNK];
		let result = same_contents(&a, &b, &mut buf_a, &mut buf_b);
		assert!(!result.same);
		assert!(
			result.bytes <= 2 * COMPARE_CHUNK as u64,
			"read {} bytes; should have stopped after the first chunk of each",
			result.bytes
		);
	}

	#[test]
	fn content_comparison_reads_identical_files_whole() {
		let tmp = tempfile::tempdir().unwrap();
		let a = tmp.path().join("a");
		let b = tmp.path().join("b");
		std::fs::write(&a, vec![b'x'; 3000]).unwrap();
		std::fs::write(&b, vec![b'x'; 3000]).unwrap();

		let mut buf_a = vec![0u8; COMPARE_CHUNK];
		let mut buf_b = vec![0u8; COMPARE_CHUNK];
		let result = same_contents(&a, &b, &mut buf_a, &mut buf_b);
		assert!(result.same);
		assert_eq!(result.bytes, 6000, "both sides, in full");
	}

	#[test]
	fn an_unreadable_side_counts_as_a_difference() {
		let tmp = tempfile::tempdir().unwrap();
		let a = tmp.path().join("a");
		std::fs::write(&a, "x").unwrap();
		let mut buf_a = vec![0u8; 64];
		let mut buf_b = vec![0u8; 64];
		assert!(!same_contents(&a, &tmp.path().join("missing"), &mut buf_a, &mut buf_b).same);
	}

	#[test]
	fn net_growth_nets_removals_off_against_copies() {
		let delta = Delta {
			copy_bytes: 1000,
			displaced_bytes: 400,
			remove_bytes: 200,
			..Default::default()
		};
		// 1000 written, 400 of it over files that were already there, 200 freed.
		assert_eq!(delta.net_growth(), 400);
		// The copy-on-write store sees every byte written, not the net.
		assert_eq!(delta.written_bytes(), 1000);
	}

	#[test]
	fn a_delta_that_frees_more_than_it_adds_needs_no_room() {
		let delta = Delta {
			copy_bytes: 100,
			displaced_bytes: 100,
			remove_bytes: 5000,
			..Default::default()
		};
		assert_eq!(delta.net_growth(), 0);
	}

	#[tokio::test]
	async fn an_empty_capture_is_refused() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		std::fs::create_dir_all(&capture).unwrap();
		let err = ensure_readable(&capture).await.unwrap_err().to_string();
		assert!(err.contains("erase the cluster"), "got: {err}");
	}

	#[tokio::test]
	async fn an_absent_capture_is_refused() {
		let err = ensure_readable(Path::new("/nonexistent/capture"))
			.await
			.unwrap_err()
			.to_string();
		assert!(err.contains("is not there"), "got: {err}");
	}
}
