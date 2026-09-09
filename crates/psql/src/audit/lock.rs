//! Advisory locks over audit files.
//!
//! A writer holds one on its current segment so that another process can tell a
//! live segment from a closed one by attempting the lock, rather than inferring
//! liveness from timestamps. Compaction holds one on a directory-level lock file.
//!
//! Nothing ever waits on these locks.
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
	/// Take the lock on an already-open file, or `None` if something else holds it.
	pub fn try_hold(file: File) -> Result<Option<Self>> {
		match FileExt::try_lock(&file) {
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
