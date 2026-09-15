//! Comparing a held capture against the live tree, and applying the difference.
//!
//! The expensive part of a restore is reading file *contents*: on the host that
//! motivated this mode, 732 GiB on each side. Reading *metadata* — `readdir`
//! plus `stat` — is cheap even on a tree that large, and it already answers
//! every structural question: what is only in the capture (copy), what is only
//! in the live tree (delete), what changed type, and what changed size.
//!
//! That leaves exactly one undecided case: an entry present in both at the same
//! size. A [diff basis](super::basis) decides that case and nothing else, which
//! is why a basis is never load-bearing for correctness — an absent, stale, or
//! wrapped one degrades to the fallback rule and the restore is still complete.
//!
//! Deletions come from the walk rather than from the basis. That matters
//! because the backends' change lists do not all report them: `btrfs subvolume
//! find-new` cannot, since a deleted file leaves no inode carrying a newer
//! generation.

use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use tracing::{debug, info, warn};

/// How much of a file is read at a time when hashing it for comparison.
const HASH_CHUNK: usize = 1024 * 1024;

/// What the walk found for one path, relative to the two tree roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
	/// Copy the capture's entry over the live tree's, which is absent, a
	/// different type, or a different size.
	Copy,
	/// Present in both as regular files of the same size. Whether the contents
	/// diverged is not knowable from metadata, so it is left to the basis or, in
	/// its absence, to [`decide_undecided`].
	SameSize {
		/// Whether the two sides also carry the same modification time.
		mtime_same: bool,
		/// The size both sides share, for estimating the work.
		len: u64,
	},
}

/// The difference between a capture and the live tree it rolls back onto.
#[derive(Debug, Default)]
pub struct Delta {
	/// Directories to create, parents before children.
	pub mkdir: Vec<PathBuf>,
	/// Entries to copy from the capture.
	pub copy: Vec<PathBuf>,
	/// Entries to remove from the live tree, children before parents.
	pub remove: Vec<PathBuf>,
	/// Entries present in both at the same size, awaiting a content decision.
	pub undecided: Vec<(PathBuf, bool, u64)>,
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
	/// Whether the walk found anything to write or remove.
	///
	/// Distinct from having nothing to consider: a tree that already matches the
	/// capture still has every same-size entry in `undecided`, and those resolve
	/// to no work at all.
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

/// Compare `capture` against `dest`, structurally.
///
/// `skip` names paths (relative to the roots) the walk must not look at, which
/// is how the interlock keeps `PG_VERSION` out of the sync so it can be written
/// back last.
pub async fn compare(capture: &Path, dest: &Path, skip: &[PathBuf]) -> Result<Delta> {
	let capture = capture.to_path_buf();
	let dest = dest.to_path_buf();
	let skip = skip.to_vec();
	tokio::task::spawn_blocking(move || {
		let mut delta = Delta::default();
		walk(&capture, &dest, Path::new(""), &skip, &mut delta)?;
		Ok(delta)
	})
	.await
	.into_diagnostic()
	.wrap_err("joining the capture comparison")?
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
		// Symlink metadata throughout: a tablespace link in `pg_tblspc` is part of
		// the captured state, and following it would compare whatever it points at.
		let meta = entry
			.path()
			.symlink_metadata()
			.into_diagnostic()
			.wrap_err_with(|| format!("stating {}", entry.path().display()))?;
		out.insert(entry.file_name(), meta);
	}
	Ok(Some(out))
}

fn walk(
	capture_root: &Path,
	dest_root: &Path,
	rel: &Path,
	skip: &[PathBuf],
	delta: &mut Delta,
) -> Result<()> {
	let Some(from) = read_side(&capture_root.join(rel))? else {
		// The capture has no such directory, so the whole live subtree goes. The
		// caller only recurses into directories present on both sides, so this is
		// a directory that vanished between the two reads.
		return Ok(());
	};
	let into = read_side(&dest_root.join(rel))?.unwrap_or_default();

	for (name, capture_meta) in &from {
		let child = rel.join(name);
		if skip.iter().any(|s| s == &child) {
			continue;
		}
		let live_meta = into.get(name);
		match classify(capture_meta, live_meta) {
			Some(Change::Copy) => {
				if capture_meta.is_dir() {
					// A directory the live tree lacks: make it, then let the recursion
					// fill it, so an empty captured directory still comes back.
					delta.mkdir.push(child.clone());
					if live_meta.is_some_and(|m| !m.is_dir()) {
						delta.remove.push(child.clone());
						delta.remove_bytes =
							delta.remove_bytes.saturating_add(live_meta.map_or(0, |m| m.len()));
					}
					walk(capture_root, dest_root, &child, skip, delta)?;
				} else {
					delta.copy_bytes = delta.copy_bytes.saturating_add(capture_meta.len());
					if let Some(live) = live_meta {
						// Replacing an entry in place: only the growth is new allocation.
						delta.displaced_bytes = delta.displaced_bytes.saturating_add(live.len());
					}
					delta.copy.push(child);
				}
			}
			Some(Change::SameSize { mtime_same, len }) => {
				delta.undecided.push((child, mtime_same, len));
			}
			None => {
				// Same directory on both sides: nothing to do here, recurse into it.
				walk(capture_root, dest_root, &child, skip, delta)?;
			}
		}
	}

	// Anything the live tree has that the capture does not was written after the
	// freeze, so it is not part of the state being rolled back to.
	for (name, live_meta) in &into {
		if from.contains_key(name) {
			continue;
		}
		let child = rel.join(name);
		if skip.iter().any(|s| s == &child) {
			continue;
		}
		if live_meta.is_dir() {
			collect_subtree_removals(dest_root, &child, delta)?;
		}
		delta.remove_bytes = delta.remove_bytes.saturating_add(live_meta.len());
		delta.remove.push(child);
	}

	Ok(())
}

/// Every entry under a directory that is going, children before the directory
/// itself, so the removal can be a plain sequence of unlinks and rmdirs and the
/// bytes it frees are counted.
fn collect_subtree_removals(dest_root: &Path, rel: &Path, delta: &mut Delta) -> Result<()> {
	let Some(entries) = read_side(&dest_root.join(rel))? else {
		return Ok(());
	};
	for (name, meta) in &entries {
		let child = rel.join(name);
		if meta.is_dir() {
			collect_subtree_removals(dest_root, &child, delta)?;
		}
		delta.remove_bytes = delta.remove_bytes.saturating_add(meta.len());
		delta.remove.push(child);
	}
	Ok(())
}

/// What to do about one capture entry given the live tree's, if any.
///
/// `None` means both sides are directories, which the caller recurses into
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
	// simply rewritten; its size is the target's length, which is not a
	// meaningful allocation.
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

/// Decide the same-size entries the walk could not, without a basis.
///
/// This is the degraded rule: identical size *and* modification time is taken
/// as identical content, so a file that diverges without changing either is
/// silently kept. It is acceptable only as the fallback — where a backend can
/// name the diverged set, that answer is authoritative and this never runs.
pub async fn decide_undecided(
	capture: &Path,
	dest: &Path,
	undecided: Vec<(PathBuf, bool, u64)>,
) -> Result<Vec<PathBuf>> {
	let capture = capture.to_path_buf();
	let dest = dest.to_path_buf();
	tokio::task::spawn_blocking(move || {
		let mut copy = Vec::new();
		let mut hashed = 0u64;
		for (rel, mtime_same, len) in undecided {
			if mtime_same {
				continue;
			}
			// Same size, different mtime: the file may have been rewritten with the
			// same content, so read both sides rather than copying on the timestamp.
			hashed = hashed.saturating_add(len);
			if !same_contents(&capture.join(&rel), &dest.join(&rel))? {
				copy.push(rel);
			}
		}
		if hashed > 0 {
			debug!(
				bytes = hashed,
				files = copy.len(),
				"hashed the same-size, different-mtime files"
			);
		}
		Ok(copy)
	})
	.await
	.into_diagnostic()
	.wrap_err("joining the content comparison")?
}

/// Whether two files hold the same bytes. Either being unreadable counts as a
/// difference, so the capture's copy is laid down rather than a doubtful one
/// kept.
fn same_contents(a: &Path, b: &Path) -> Result<bool> {
	fn hash(path: &Path) -> Result<Option<blake3::Hash>> {
		use std::io::Read as _;
		let mut file = match std::fs::File::open(path) {
			Ok(file) => file,
			Err(err) => {
				debug!("could not read {} to compare it: {err}", path.display());
				return Ok(None);
			}
		};
		let mut hasher = blake3::Hasher::new();
		let mut buf = vec![0u8; HASH_CHUNK];
		loop {
			match file.read(&mut buf) {
				Ok(0) => break,
				Ok(n) => {
					hasher.update(&buf[..n]);
				}
				Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
				Err(err) => {
					debug!("could not read {} to compare it: {err}", path.display());
					return Ok(None);
				}
			}
		}
		Ok(Some(hasher.finalize()))
	}
	match (hash(a)?, hash(b)?) {
		(Some(a), Some(b)) => Ok(a == b),
		_ => Ok(false),
	}
}

/// Lay the delta down: remove what the capture does not have, make the
/// directories it does, and copy its entries over the live tree's.
///
/// Ordered removals first, so a path that changed type has its old entry out of
/// the way before the new one is written, and so a tree that grew since the
/// freeze gives its space back before the copy asks for any.
pub async fn apply(capture: &Path, dest: &Path, delta: &Delta, copy: &[PathBuf]) -> Result<()> {
	// Children were pushed before their parents, so removing in order unlinks a
	// directory's contents before the directory.
	for rel in &delta.remove {
		let path = dest.join(rel);
		let meta = match path.symlink_metadata() {
			Ok(meta) => meta,
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
			Err(err) => {
				return Err(err)
					.into_diagnostic()
					.wrap_err_with(|| format!("stating {} to remove it", path.display()));
			}
		};
		let removed = if meta.is_dir() {
			tokio::fs::remove_dir(&path).await
		} else {
			tokio::fs::remove_file(&path).await
		};
		removed
			.into_diagnostic()
			.wrap_err_with(|| format!("removing {}", path.display()))?;
	}

	// Parents were pushed before their children.
	for rel in &delta.mkdir {
		let path = dest.join(rel);
		tokio::fs::create_dir_all(&path)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", path.display()))?;
		copy_metadata(&capture.join(rel), &path).await?;
	}

	let total = copy.len();
	let mut done = 0usize;
	let mut copied_bytes = 0u64;
	let mut last_report = std::time::Instant::now();
	for rel in copy {
		copied_bytes = copied_bytes.saturating_add(copy_entry(&capture.join(rel), &dest.join(rel)).await?);
		done += 1;
		// A large sync is otherwise a silent wait, and this one is running against
		// a cluster that is down.
		if last_report.elapsed() >= std::time::Duration::from_secs(30) {
			info!(
				done,
				total,
				bytes = copied_bytes,
				"copying the divergence into place"
			);
			last_report = std::time::Instant::now();
		}
	}

	// A directory's own timestamps change as its contents are written, so set
	// them after the copies rather than when the directory is made.
	for rel in delta.mkdir.iter().rev() {
		copy_metadata(&capture.join(rel), &dest.join(rel)).await?;
	}

	Ok(())
}

/// Copy one entry from the capture over the live tree's, returning the bytes
/// written. The parent is created first: a file can be in the copy set without
/// its directory being in the mkdir set when the directory exists but the file
/// does not.
pub async fn copy_entry(from: &Path, to: &Path) -> Result<u64> {
	let meta = from
		.symlink_metadata()
		.into_diagnostic()
		.wrap_err_with(|| format!("stating {}", from.display()))?;

	if let Some(parent) = to.parent() {
		tokio::fs::create_dir_all(parent)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", parent.display()))?;
	}

	// Whatever is there now may be the wrong type, or a symlink that a plain
	// write would follow out of the tree.
	match to.symlink_metadata() {
		Ok(existing) if existing.is_dir() => {
			tokio::fs::remove_dir_all(to)
				.await
				.into_diagnostic()
				.wrap_err_with(|| format!("removing {} to replace it", to.display()))?;
		}
		Ok(existing) if existing.is_symlink() || meta.is_symlink() => {
			let _ = existing;
			tokio::fs::remove_file(to)
				.await
				.into_diagnostic()
				.wrap_err_with(|| format!("removing {} to replace it", to.display()))?;
		}
		_ => {}
	}

	if meta.is_symlink() {
		let target = tokio::fs::read_link(from)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("reading the link {}", from.display()))?;
		symlink(&target, to).await?;
		return Ok(0);
	}
	if meta.is_dir() {
		tokio::fs::create_dir_all(to)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", to.display()))?;
		copy_metadata(from, to).await?;
		return Ok(0);
	}
	if !meta.is_file() {
		// Sockets and the like are live-process artefacts, not captured state.
		debug!("skipping {} ({:?})", from.display(), meta.file_type());
		return Ok(0);
	}

	tokio::fs::copy(from, to)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("copying {} to {}", from.display(), to.display()))?;
	copy_metadata(from, to).await?;
	Ok(meta.len())
}

#[cfg(unix)]
async fn symlink(target: &Path, at: &Path) -> Result<()> {
	tokio::fs::symlink(target, at)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("linking {} -> {}", at.display(), target.display()))
}

#[cfg(windows)]
async fn symlink(target: &Path, at: &Path) -> Result<()> {
	// The capture's own entry says which kind it is; a directory link made as a
	// file link resolves to nothing.
	let result = if target.is_dir() {
		tokio::fs::symlink_dir(target, at).await
	} else {
		tokio::fs::symlink_file(target, at).await
	};
	result
		.into_diagnostic()
		.wrap_err_with(|| format!("linking {} -> {}", at.display(), target.display()))
}

/// Carry the capture's permissions, ownership, and modification time onto the
/// entry written from it, so the restored tree is the captured tree rather than
/// one wearing the restoring process's umask and clock.
///
/// Best-effort on each attribute: postgres is started as its service account
/// afterwards and the restore fixes ownership across the whole tree at that
/// point, so a single failure here is not worth abandoning a restore over.
async fn copy_metadata(from: &Path, to: &Path) -> Result<()> {
	let from = from.to_path_buf();
	let to = to.to_path_buf();
	tokio::task::spawn_blocking(move || {
		let Ok(meta) = from.symlink_metadata() else {
			return;
		};
		// A symlink's own metadata is not settable through these APIs, and the
		// target it points at must not be touched in its place.
		if meta.is_symlink() {
			return;
		}
		#[cfg(unix)]
		{
			use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
			if let Err(err) = std::fs::set_permissions(
				&to,
				std::fs::Permissions::from_mode(meta.permissions().mode()),
			) {
				debug!("could not set the mode of {}: {err}", to.display());
			}
			if let Err(err) = std::os::unix::fs::chown(&to, Some(meta.uid()), Some(meta.gid())) {
				debug!("could not set the owner of {}: {err}", to.display());
			}
		}
		#[cfg(not(unix))]
		{
			if let Err(err) = std::fs::set_permissions(&to, meta.permissions()) {
				debug!("could not set the permissions of {}: {err}", to.display());
			}
		}
		if let Ok(mtime) = meta.modified()
			&& let Err(err) = filetime::set_file_mtime(&to, filetime::FileTime::from(mtime))
		{
			debug!("could not set the mtime of {}: {err}", to.display());
		}
	})
	.await
	.into_diagnostic()
	.wrap_err("joining the metadata copy")
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
		Err(err) => {
			warn!("the capture at {} could not be read", capture.display());
			Err(err)
		}
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

	/// The two trees and the delta between them, for a test that only cares
	/// about the comparison.
	async fn delta_of(
		build: impl FnOnce(&Path, &Path),
	) -> (tempfile::TempDir, PathBuf, PathBuf, Delta) {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		std::fs::create_dir_all(&capture).unwrap();
		std::fs::create_dir_all(&dest).unwrap();
		build(&capture, &dest);
		let delta = compare(&capture, &dest, &[]).await.unwrap();
		(tmp, capture, dest, delta)
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
		// Same size on both sides, so the content decision is deferred rather than
		// resolved by the walk.
		assert!(delta.copy.is_empty());
		assert!(delta.remove.is_empty());
		assert_eq!(delta.undecided.len(), 2);
	}

	#[tokio::test]
	async fn a_file_written_since_the_freeze_is_removed() {
		let (_tmp, _c, _d, delta) = delta_of(|capture, dest| {
			write(capture, "PG_VERSION", "16");
			write(dest, "PG_VERSION", "16");
			write(dest, "base/1/9999", "written after the capture");
		})
		.await;
		// The directory and the file both go: neither is part of the captured state.
		assert!(delta.remove.contains(&PathBuf::from("base/1/9999")));
		assert!(delta.remove.contains(&PathBuf::from("base")));
		// Children are removed before their parents.
		let file = delta.remove.iter().position(|p| p.ends_with("9999")).unwrap();
		let dir = delta.remove.iter().position(|p| p == Path::new("base")).unwrap();
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
		assert!(delta.remove.contains(&PathBuf::from("PG_VERSION")));
	}

	#[tokio::test]
	async fn a_size_change_is_decided_by_the_walk_alone() {
		let (_tmp, _c, _d, delta) = delta_of(|capture, dest| {
			write(capture, "base/1/2345", "grown since the freeze");
			write(dest, "base/1/2345", "short");
		})
		.await;
		assert_eq!(delta.copy, vec![PathBuf::from("base/1/2345")]);
		assert!(delta.undecided.is_empty());
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
		let delta = compare(&capture, &dest, &[PathBuf::from("PG_VERSION")])
			.await
			.unwrap();
		assert!(!delta.has_work(), "the interlock's file must not be synced");
		assert!(delta.undecided.is_empty(), "nor considered");
	}

	#[tokio::test]
	async fn same_size_different_mtime_but_same_content_is_not_copied() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "f", "sixteen bytes!!!");
		write(&dest, "f", "sixteen bytes!!!");
		filetime::set_file_mtime(dest.join("f"), filetime::FileTime::from_unix_time(1, 0)).unwrap();

		let delta = compare(&capture, &dest, &[]).await.unwrap();
		assert_eq!(delta.undecided.len(), 1);
		assert!(!delta.undecided[0].1, "the mtimes differ");

		let copy = decide_undecided(&capture, &dest, delta.undecided).await.unwrap();
		assert!(copy.is_empty(), "identical contents must not be copied");
	}

	#[tokio::test]
	async fn same_size_different_mtime_and_different_content_is_copied() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let dest = tmp.path().join("dest");
		write(&capture, "f", "sixteen bytes!!!");
		write(&dest, "f", "SIXTEEN BYTES!!!");
		filetime::set_file_mtime(dest.join("f"), filetime::FileTime::from_unix_time(1, 0)).unwrap();

		let delta = compare(&capture, &dest, &[]).await.unwrap();
		let copy = decide_undecided(&capture, &dest, delta.undecided).await.unwrap();
		assert_eq!(copy, vec![PathBuf::from("f")]);
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

		let delta = compare(&capture, &dest, &[]).await.unwrap();
		assert!(delta.undecided[0].1, "the mtimes match");
		let copy = decide_undecided(&capture, &dest, delta.undecided).await.unwrap();
		assert!(copy.is_empty());
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

		let delta = compare(&capture, &dest, &[]).await.unwrap();
		let extra = decide_undecided(&capture, &dest, delta.undecided.clone())
			.await
			.unwrap();
		let copy: Vec<PathBuf> = delta.copy.iter().cloned().chain(extra).collect();
		apply(&capture, &dest, &delta, &copy).await.unwrap();

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

		for _ in 0..2 {
			let delta = compare(&capture, &dest, &[]).await.unwrap();
			let extra = decide_undecided(&capture, &dest, delta.undecided.clone())
				.await
				.unwrap();
			let copy: Vec<PathBuf> = delta.copy.iter().cloned().chain(extra).collect();
			apply(&capture, &dest, &delta, &copy).await.unwrap();
		}

		// The second pass has nothing left to do, which is what makes a failed
		// restore resumable rather than restartable.
		let delta = compare(&capture, &dest, &[]).await.unwrap();
		assert!(delta.copy.is_empty() && delta.remove.is_empty() && delta.mkdir.is_empty());
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

		let delta = compare(&capture, &dest, &[]).await.unwrap();
		apply(&capture, &dest, &delta, &delta.copy.clone()).await.unwrap();

		let link = dest.join("pg_tblspc/16400");
		assert!(link.symlink_metadata().unwrap().is_symlink());
		assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("/mnt/fast/tblspc"));
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

		let delta = compare(&capture, &dest, &[]).await.unwrap();
		apply(&capture, &dest, &delta, &delta.copy.clone()).await.unwrap();
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

		let delta = compare(&capture, &dest, &[]).await.unwrap();
		apply(&capture, &dest, &delta, &delta.copy.clone()).await.unwrap();

		assert!(dest.join("thing").is_file());
		assert_eq!(
			std::fs::read_to_string(dest.join("thing")).unwrap(),
			"a file in the capture"
		);
	}

	#[tokio::test]
	async fn an_absent_capture_is_refused() {
		let err = ensure_readable(Path::new("/nonexistent/capture"))
			.await
			.unwrap_err()
			.to_string();
		assert!(err.contains("is not there"), "got: {err}");
	}

	#[tokio::test]
	async fn net_growth_nets_removals_off_against_copies() {
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

	#[tokio::test]
	async fn a_delta_that_frees_more_than_it_adds_needs_no_room() {
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
}
