//! The NTFS change journal, read to find what diverged from a shadow copy.
//!
//! A shadow copy is a whole-volume snapshot, so restoring from one in place has
//! to know which files changed since it was taken. NTFS already keeps that:
//! every change to a file on the volume appends a record to the volume's update
//! sequence number (USN) journal, naming the file and why it changed. Reading it
//! costs a pass over the journal rather than a read of both trees.
//!
//! The journal is a **fixed-size ring**. Once it is full the oldest records are
//! discarded to make room, so a journal that has wrapped past the position a
//! capture recorded no longer holds the whole answer — and a partial answer is
//! not an answer. That, and the journal having been deleted and recreated since
//! (which gives it a new id and restarts its numbering), are both detected by
//! [`coverage`], which yields nothing rather than a subset. The caller then
//! compares the trees.
//!
//! The journal is read through `usn-journal-rs`, which wraps the `DeviceIoControl`
//! calls and reconstructs a record's full path from its parent's file id. This
//! workspace forbids unsafe code, so the alternative would be driving `fsutil`
//! and parsing output meant for people.
//!
//! Verify on a real host before relying on this as more than an optimisation.

#[cfg(any(windows, test))]
use std::path::Path;
#[cfg(windows)]
use std::{collections::BTreeSet, path::PathBuf};

#[cfg(windows)]
use tracing::{debug, warn};

// Only Windows reads a change journal, so the four items below are built there
// and under test. Deciding whether a recorded position is still answerable is
// the part that would quietly leave files diverged if it were wrong, so it is
// worth checking on every platform CI runs rather than only on the one it runs
// on — hence `test` rather than `windows` alone.

#[cfg(any(windows, test))]
/// Where a volume's journal stood at a moment, enough to tell later whether it
/// still covers that moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
	/// The journal's identity. A journal deleted and recreated gets a new one,
	/// and its numbering restarts, so a position recorded against the old one
	/// means nothing against the new.
	pub journal_id: u64,
	/// The sequence number the next record will be written at.
	pub usn: i64,
}

#[cfg(any(windows, test))]
/// Whether the journal as it stands still accounts for a recorded position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
	/// Every change since the position is still in the journal.
	Covered,
	/// The journal has been deleted and recreated since, so its numbering has
	/// nothing to do with the recorded position.
	Recreated,
	/// The ring has overwritten the records from the position onwards. What is
	/// left is a suffix of the changes, and a suffix is not the set.
	Wrapped,
}

/// Whether a position recorded at a capture is still answerable.
///
/// Kept separate from the reading so it can be reasoned about — and tested —
/// away from a Windows volume. Getting it wrong in the permissive direction is
/// the one failure that matters: it would leave files diverged.
#[cfg(any(windows, test))]
pub fn coverage(since: Position, journal_id: u64, first_usn: i64) -> Coverage {
	if journal_id != since.journal_id {
		Coverage::Recreated
	} else if since.usn < first_usn {
		Coverage::Wrapped
	} else {
		Coverage::Covered
	}
}

#[cfg(any(windows, test))]
/// The drive letter `volume` names, as the journal API wants it.
fn drive_letter(volume: &Path) -> Option<char> {
	volume
		.to_str()?
		.chars()
		.next()
		.filter(char::is_ascii_alphabetic)
}

/// Where the volume's journal stands now.
///
/// Called when a capture freezes, so a later restore has something to diff
/// against. `None` where the volume has no active journal, which is a supported
/// state rather than an error: a later restore compares the trees itself. The
/// journal is never created here — it is host-wide configuration that backups
/// share with everything else on the volume, so turning it on is an operator
/// decision, as shadow storage sizing is.
#[cfg(windows)]
pub async fn position(volume: &Path) -> Option<Position> {
	let volume = volume.to_path_buf();
	tokio::task::spawn_blocking(move || {
		let letter = drive_letter(&volume)?;
		let handle = usn_journal_rs::volume::Volume::from_drive_letter(letter)
			.inspect_err(|err| debug!("no change journal on {letter}: {err}"))
			.ok()?;
		let data = handle
			.journal()
			.query(false)
			.inspect_err(|err| debug!("no change journal on {letter}: {err}"))
			.ok()?;
		let position = Position {
			journal_id: data.journal_id,
			usn: data.next_usn,
		};
		debug!(?position, "read {letter}'s change journal position");
		Some(position)
	})
	.await
	.ok()
	.flatten()
}

/// The paths on `volume` that changed since `since`, relative to `root`.
///
/// `None` means the journal cannot answer — it is inactive, it has been
/// recreated, it has wrapped past the recorded position, or it could not be
/// read. The caller must then compare the trees rather than read an empty set as
/// "nothing changed": those are opposite conclusions.
#[cfg(windows)]
pub async fn changed_since(
	volume: &Path,
	since: Position,
	root: &Path,
) -> Option<BTreeSet<PathBuf>> {
	use usn_journal_rs::{journal::EnumOptions, volume::Volume};

	let volume = volume.to_path_buf();
	let root = root.to_path_buf();
	tokio::task::spawn_blocking(move || {
		let letter = drive_letter(&volume)?;
		let handle = Volume::from_drive_letter(letter)
			.inspect_err(|err| {
				warn!("{letter} has no readable change journal, so the restore compared the trees: {err}")
			})
			.ok()?;

		let data = handle.journal().query(false).ok()?;
		match coverage(since, data.journal_id, data.first_usn) {
			Coverage::Covered => {}
			Coverage::Recreated => {
				warn!(
					"{letter}'s change journal has been recreated since the capture, \
					 so it no longer says what changed"
				);
				return None;
			}
			Coverage::Wrapped => {
				warn!(
					"{letter}'s change journal has wrapped past the capture, so it no \
					 longer holds every change since"
				);
				return None;
			}
		}

		let journal = handle.journal();
		let entries = journal
			.iter_with_options(EnumOptions {
				start_usn: since.usn,
				// Every reason: the question is only whether a file might differ, and
				// every reason means it might. An extra path costs one comparison.
				..EnumOptions::default()
			})
			.ok()?;

		let mut resolver = handle.path_resolver_with_cache();
		let mut paths = BTreeSet::new();
		let mut records = 0usize;
		for entry in entries {
			// A read that fails partway has produced a prefix of the changes, which
			// is no more an answer than a wrapped journal's suffix.
			let entry = entry
				.inspect_err(|err| {
					warn!(
						"{letter}'s change journal stopped partway, so the restore \
						 compared the trees instead: {err}"
					)
				})
				.ok()?;
			// A file deleted since the capture cannot be resolved, and does not need
			// to be: the walk finds a deletion structurally.
			let Some(path) = resolver.resolve_path(&entry) else {
				continue;
			};
			records += 1;
			if let Ok(rel) = path.strip_prefix(&root) {
				paths.insert(rel.to_path_buf());
			}
		}

		// A volume-wide journal that resolved plenty of paths, none of which lands
		// under the tree being restored, is the signature of the paths being in a
		// different form than `root` — volume-relative against absolute, an
		// extended-length prefix, a short name. That is indistinguishable here from
		// "nothing under the cluster changed", and the two lead to opposite
		// actions: one is a correct no-op, the other silently leaves the whole tree
		// diverged. Until this path is confirmed on a real host, decline.
		if paths.is_empty() && records > 0 {
			warn!(
				records,
				root = %root.display(),
				"{letter}'s change journal resolved paths but none under the tree being \
				 restored, so the restore compared the trees instead",
			);
			return None;
		}

		debug!(
			changed = paths.len(),
			"{letter}'s change journal named what diverged from the capture"
		);
		Some(paths)
	})
	.await
	.ok()
	.flatten()
}

#[cfg(test)]
mod tests {
	use super::*;

	const AT_CAPTURE: Position = Position {
		journal_id: 0x01d5_f4e2_c3b1_a098,
		usn: 0x0012_3456,
	};

	#[test]
	fn a_journal_still_holding_the_capture_can_answer() {
		assert_eq!(
			coverage(AT_CAPTURE, AT_CAPTURE.journal_id, 0x0001_0000),
			Coverage::Covered
		);
	}

	#[test]
	fn a_position_at_the_very_first_record_is_still_covered() {
		// The boundary is inclusive: the record at `first_usn` has not been
		// discarded, so a capture recorded there is answerable.
		assert_eq!(
			coverage(AT_CAPTURE, AT_CAPTURE.journal_id, AT_CAPTURE.usn),
			Coverage::Covered
		);
	}

	#[test]
	fn a_ring_that_wrapped_past_the_capture_cannot_answer() {
		assert_eq!(
			coverage(AT_CAPTURE, AT_CAPTURE.journal_id, AT_CAPTURE.usn + 1),
			Coverage::Wrapped
		);
	}

	#[test]
	fn a_recreated_journal_cannot_answer_however_its_numbering_looks() {
		// Its sequence numbers restart, so a recorded position can land anywhere
		// in the new journal's range and mean nothing. The id is what settles it.
		assert_eq!(coverage(AT_CAPTURE, 999, 0), Coverage::Recreated);
		assert_eq!(coverage(AT_CAPTURE, 999, i64::MAX), Coverage::Recreated);
	}

	#[test]
	fn reads_the_drive_letter_the_journal_api_wants() {
		assert_eq!(drive_letter(Path::new("C:")), Some('C'));
		assert_eq!(drive_letter(Path::new(r"D:\")), Some('D'));
		assert_eq!(drive_letter(Path::new(r"\\?\Volume{abc}")), None);
		assert_eq!(drive_letter(Path::new("")), None);
	}
}
