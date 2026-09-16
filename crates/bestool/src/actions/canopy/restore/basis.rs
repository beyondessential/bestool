//! Asking the filesystem what diverged from a capture, rather than looking.
//!
//! The [walk](super::sync) settles every structural question from metadata
//! alone, and leaves exactly one open: an entry present in both trees at the
//! same size. A basis answers that one, from the change history the filesystem
//! already keeps — btrfs's transaction generations, NTFS's update sequence
//! numbers — without reading a byte of either tree's contents.
//!
//! A basis is **never** load-bearing for correctness. Each backend can decline
//! for reasons that are ordinary rather than exceptional: the capture predates
//! the record carrying a position, the tooling is not installed, a ring buffer
//! wrapped. Declining falls the restore back on [`super::sync::decide_undecided`],
//! which is slower and, on files whose size and modification time both survive a
//! divergence, weaker — so the mode says which of the two it got.

use std::{
	collections::BTreeSet,
	path::{Path, PathBuf},
};

use tracing::{info, warn};

use super::sync::Decide;
use crate::actions::canopy::backup::hold::{DivergenceMark, HoldRecord};

#[cfg(unix)]
mod btrfs;
#[cfg(unix)]
mod lvm;

/// What is known about which entries diverged.
#[derive(Debug)]
pub enum Basis {
	/// The filesystem named the set, and it is complete: an entry outside it has
	/// not been written since the capture, whatever its size and timestamp say.
	Named {
		/// Paths relative to the roots being compared.
		paths: BTreeSet<PathBuf>,
		/// How the set was obtained, for the operator-facing log.
		how: &'static str,
	},
	/// Nothing to go on. The same-size entries are decided by comparing them.
	Unavailable {
		/// Why, in a phrase that completes "…, so the restore compared the trees".
		why: String,
	},
}

impl Basis {
	/// How the walk should settle a same-size entry.
	///
	/// The named set is only reachable through this, so there is no way to ask
	/// whether the filesystem named a path without first establishing that it
	/// answered at all — the question is meaningless otherwise, and a wrong
	/// answer to it is a file left diverged.
	pub fn into_decision(self) -> Decide {
		match self {
			Self::Named { paths, .. } => Decide::Named(paths),
			Self::Unavailable { .. } => Decide::Compare,
		}
	}

	/// Whether the filesystem answered, for the operator-facing report.
	pub fn is_named(&self) -> bool {
		matches!(self, Self::Named { .. })
	}

	fn unavailable(why: impl Into<String>) -> Self {
		Self::Unavailable { why: why.into() }
	}
}

/// Work out what the capture's backend can say about what diverged.
///
/// `live` is the tree being restored onto, which is what the change history is
/// read against — the capture itself is frozen and has no history since.
pub async fn resolve(record: &HoldRecord, live: &Path) -> Basis {
	let basis = match &record.diverged_since {
		#[cfg(unix)]
		Some(DivergenceMark::BtrfsGeneration {
			generation,
			subvolume,
		}) => match subvolume {
			Some(subvolume) => btrfs::changed_since(*generation, subvolume, live).await,
			// A generation with no subvolume to check it against cannot be told
			// apart from one counted on a different filesystem, which would answer
			// cleanly and wrongly.
			None => Basis::unavailable(
				"the capture recorded a btrfs generation without the subvolume it was counted on",
			),
		},
		#[cfg(windows)]
		Some(DivergenceMark::UsnJournal {
			volume,
			journal_id,
			usn,
		}) => {
			let position = crate::actions::canopy::backup::postgresql::usn::Position {
				journal_id: *journal_id,
				usn: *usn,
			};
			match crate::actions::canopy::backup::postgresql::usn::changed_since(
				volume, position, live,
			)
			.await
			{
				Some(paths) => Basis::Named {
					paths,
					how: "the NTFS change journal",
				},
				None => Basis::unavailable("the NTFS change journal could not answer"),
			}
		}
		// A mark for a platform this build cannot read is not an error: the same
		// hold record is read by whatever bestool runs next on the host, and a
		// mark it cannot use is simply a mark it compares the trees without.
		#[cfg(not(unix))]
		Some(DivergenceMark::BtrfsGeneration { .. }) => {
			Basis::unavailable("the capture's btrfs generation cannot be read here")
		}
		#[cfg(not(windows))]
		Some(DivergenceMark::UsnJournal { .. }) => {
			Basis::unavailable("the capture's change journal cannot be read here")
		}
		None => Basis::unavailable(format!(
			"the {} capture recorded no position in its filesystem's change history",
			record.capture.backend()
		)),
	};

	match &basis {
		Basis::Named { paths, how } => info!(
			changed = paths.len(),
			"{how} named what diverged from the capture"
		),
		Basis::Unavailable { why } => warn!(
			"{why}, so the restore compared the trees; files whose size and \
			 modification time both survived a change are kept as they are"
		),
	}
	basis
}

/// A cheap, exact count of the bytes that diverged, where the backend can give
/// one without reading the trees.
///
/// Distinct from the basis: thin LVM keeps its history as block mappings with no
/// way back to the file that owns a block, so it cannot say *which* entries
/// diverged, but it can say *how much* — which is exactly what the space gate
/// needs. `None` leaves the gate to the walk's own estimate.
pub async fn divergence_bytes(record: &HoldRecord, live: &Path) -> Option<u64> {
	#[cfg(unix)]
	{
		lvm::diverged_bytes(record, live).await
	}
	#[cfg(not(unix))]
	{
		let _ = (record, live);
		None
	}
}
