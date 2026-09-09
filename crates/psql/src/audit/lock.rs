//! Locks over audit files.
//!
//! A writer holds one on its current segment so that another process can tell a
//! live segment from a closed one by attempting the lock, rather than inferring
//! liveness from timestamps. Compaction holds one on a directory-level lock file.
//!
//! Nothing ever waits on these locks.
//!
//! A writer takes a *shared* lock and anything asking whether a file is live
//! tries an *exclusive* one, which fails for as long as the writer holds its
//! share. The two are not interchangeable: Windows byte-range locks are
//! enforced rather than advisory, so an exclusive lock on a live segment would
//! stop every reader, and the log is meant to be readable as it is written.
//!
//! spec: AUD-STO, AUD-RET

use std::{
	fs::{File, OpenOptions},
	path::Path,
};

use fs4::FileExt;
use miette::{IntoDiagnostic as _, Result, WrapErr as _};
use tracing::trace;

/// A held advisory lock, released when dropped.
#[derive(Debug)]
pub struct Lock {
	file: File,
}

impl Lock {
	/// Take the file exclusively, or `None` if anything else holds it at all.
	///
	/// This is how a caller asks whether a file is live, and how it stops one
	/// becoming live under it.
	pub fn try_hold(file: File) -> Result<Option<Self>> {
		Self::taken(FileExt::try_lock(&file), file)
	}

	/// Take a share of the file, or `None` if it is held exclusively.
	///
	/// A writer holds its segment this way: an exclusive attempt then fails, so
	/// the file reads as live, while readers are left alone.
	pub fn try_share(file: File) -> Result<Option<Self>> {
		Self::taken(FileExt::try_lock_shared(&file), file)
	}

	fn taken(
		outcome: std::result::Result<(), fs4::TryLockError>,
		file: File,
	) -> Result<Option<Self>> {
		match outcome {
			Ok(()) => Ok(Some(Self { file })),
			Err(fs4::TryLockError::WouldBlock) => Ok(None),
			Err(fs4::TryLockError::Error(err)) => Err(err).into_diagnostic(),
		}
	}

	/// Take the lock on the directory-level lock file, creating it if needed.
	pub fn try_directory(dir: &Path) -> Result<Option<Self>> {
		let path = dir.join(super::paths::DIRECTORY_LOCK);
		let file = OpenOptions::new()
			.create(true)
			.read(true)
			.write(true)
			.truncate(false)
			.open(&path)
			.into_diagnostic()
			.wrap_err_with(|| format!("opening audit lock file {}", path.display()))?;
		Self::try_hold(file)
	}

	/// The locked file, for a caller that appends to it.
	pub fn file_mut(&mut self) -> &mut File {
		&mut self.file
	}
}

impl Drop for Lock {
	fn drop(&mut self) {
		if let Err(err) = FileExt::unlock(&self.file) {
			trace!(?err, "releasing audit lock");
		}
	}
}

/// Whether nothing is writing the file at `path`.
///
/// Answers only for the instant it is asked: anything about to act on the file
/// takes and holds the lock itself rather than asking first.
#[cfg(test)]
pub fn is_free(path: &Path) -> bool {
	let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
		return false;
	};
	matches!(Lock::try_hold(file), Ok(Some(_)))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_held_lock_excludes_a_second_holder() {
		let dir = tempfile::tempdir().unwrap();
		let held = Lock::try_directory(dir.path()).unwrap();
		assert!(held.is_some());

		// The same process taking the same advisory lock twice through separate
		// file handles is the case compaction and a writer race on.
		let path = dir.path().join(super::super::paths::DIRECTORY_LOCK);
		let second = OpenOptions::new()
			.read(true)
			.write(true)
			.open(&path)
			.unwrap();
		assert!(Lock::try_hold(second).unwrap().is_none());

		drop(held);
		assert!(Lock::try_directory(dir.path()).unwrap().is_some());
	}

	#[test]
	fn a_shared_hold_still_reads_as_taken() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("segment");
		std::fs::write(&path, b"records").unwrap();

		let file = OpenOptions::new()
			.read(true)
			.write(true)
			.open(&path)
			.unwrap();
		let writer = Lock::try_share(file).unwrap().unwrap();

		// Anything asking whether the file is live gets told that it is.
		assert!(!is_free(&path));

		// And it can still be read while the writer holds it, which is what the
		// log promises and what Windows would otherwise prevent.
		assert_eq!(std::fs::read(&path).unwrap(), b"records");

		drop(writer);
		assert!(is_free(&path));
	}

	#[test]
	fn a_free_file_reads_as_free() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("segment");
		std::fs::write(&path, b"").unwrap();
		assert!(is_free(&path));

		let file = OpenOptions::new()
			.read(true)
			.write(true)
			.open(&path)
			.unwrap();
		let held = Lock::try_hold(file).unwrap().unwrap();
		assert!(!is_free(&path));
		drop(held);
		assert!(is_free(&path));
	}

	#[test]
	fn a_missing_file_is_not_free() {
		let dir = tempfile::tempdir().unwrap();
		assert!(!is_free(&dir.path().join("absent")));
	}
}
