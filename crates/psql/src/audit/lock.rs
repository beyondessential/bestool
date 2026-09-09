//! Locks over audit files.
//!
//! A writer holds one on its current segment so that another process can tell a
//! live segment from a closed one by attempting the lock, rather than inferring
//! liveness from timestamps. Compaction holds one on a directory-level lock file.
//!
//! Nothing ever waits on these locks.
//!
//! No lock is ever taken over a segment's own bytes. Windows byte-range locks
//! are enforced rather than advisory: an exclusive one would stop every reader
//! of a live segment, and a shared one would stop even the writer holding it
//! from appending. Each lock is therefore taken on a file that holds nothing and
//! that nobody reads, which answers the only question being asked — is a session
//! writing this segment? — without standing in anyone's way.
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
	/// Take an already-open file exclusively, or `None` if anything else holds it.
	pub fn try_hold(file: File) -> Result<Option<Self>> {
		match FileExt::try_lock(&file) {
			Ok(()) => Ok(Some(Self { file })),
			Err(fs4::TryLockError::WouldBlock) => Ok(None),
			Err(fs4::TryLockError::Error(err)) => Err(err).into_diagnostic(),
		}
	}

	/// Take the lock that stands for a segment being live.
	///
	/// The lock is on a file beside the segment, not on the segment, so a live
	/// segment stays readable and its writer stays able to append.
	pub fn try_segment(segment: &Path) -> Result<Option<Self>> {
		Self::at(&super::paths::lock_of(segment))
	}

	fn at(path: &Path) -> Result<Option<Self>> {
		let file = OpenOptions::new()
			.create(true)
			.read(true)
			.write(true)
			.truncate(false)
			.open(path)
			.into_diagnostic()
			.wrap_err_with(|| format!("opening audit lock file {}", path.display()))?;
		Self::try_hold(file)
	}

	/// Take the lock on the directory-level lock file, creating it if needed.
	pub fn try_directory(dir: &Path) -> Result<Option<Self>> {
		Self::at(&dir.join(super::paths::DIRECTORY_LOCK))
	}
}

impl Drop for Lock {
	fn drop(&mut self) {
		if let Err(err) = FileExt::unlock(&self.file) {
			trace!(?err, "releasing audit lock");
		}
	}
}

/// Whether no session is writing the segment at `path`.
///
/// Answers only for the instant it is asked: anything about to act on the
/// segment takes and holds the lock itself rather than asking first.
#[cfg(test)]
pub fn is_free(path: &Path) -> bool {
	matches!(Lock::try_segment(path), Ok(Some(_)))
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
	fn a_held_segment_reads_as_taken_but_stays_usable() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("segment");
		std::fs::write(&path, b"records").unwrap();

		let held = Lock::try_segment(&path).unwrap().unwrap();
		assert!(!is_free(&path));

		// The segment itself is never locked, so anyone can still read it and
		// its writer can still append. Locking the segment's own bytes would
		// prevent one or the other on Windows.
		assert_eq!(std::fs::read(&path).unwrap(), b"records");
		let mut appending = OpenOptions::new().append(true).open(&path).unwrap();
		std::io::Write::write_all(&mut appending, b" more").unwrap();
		drop(appending);
		assert_eq!(std::fs::read(&path).unwrap(), b"records more");

		drop(held);
		assert!(is_free(&path));
	}

	#[test]
	fn a_segment_nothing_holds_reads_as_free() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("segment");
		std::fs::write(&path, b"").unwrap();
		assert!(is_free(&path));
	}
}
