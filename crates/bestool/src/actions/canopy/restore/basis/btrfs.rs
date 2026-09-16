//! What btrfs says has changed since a snapshot's transaction generation.
//!
//! Every write to a subvolume lands in a transaction, and transactions carry
//! monotonic generation numbers. A snapshot froze the subvolume at one of them,
//! which the hold record carries, so `btrfs subvolume find-new` against the
//! *live* subvolume lists the files written since — from metadata, without
//! reading any file's contents.
//!
//! `find-new` cannot report deletions: a file removed since the snapshot leaves
//! no inode carrying a newer generation. That is not a gap here, because the
//! [walk](super::super::sync) finds deletions structurally and the basis is only
//! ever asked about entries present in both trees.

use std::{collections::BTreeSet, path::Path};

use tracing::debug;

use super::{Basis, super::blockdev};

/// The paths under `live` written since `generation` on subvolume `expected`.
pub async fn changed_since(generation: u64, expected: &str, live: &Path) -> Basis {
	// `find-new` reports paths relative to a *subvolume* root, and the live tree
	// is usually a directory inside one, so its answers have to be re-based.
	let Some((subvol, uuid)) = enclosing_subvolume(live).await else {
		return Basis::unavailable("the live tree is not on a btrfs subvolume this can read");
	};

	// A generation is a count on one subvolume. Asked of another — a cluster
	// rebuilt since, a `--target` pointing at a different volume, a filesystem
	// recreated so its generations started again from low numbers — `find-new`
	// does not fail. It answers, plausibly, about writes that have nothing to do
	// with this capture, and a named basis is then believed.
	if uuid != expected {
		return Basis::unavailable(format!(
			"the live tree is on btrfs subvolume {uuid}, not the {expected} the capture \
			 counted its generation on"
		));
	}
	let Ok(rel) = live.strip_prefix(&subvol) else {
		return Basis::unavailable("the live tree is not under the subvolume btrfs reports");
	};

	let Some(output) = blockdev::capture(
		"btrfs",
		&[
			"subvolume",
			"find-new",
			"--",
			&subvol.to_string_lossy(),
			&generation.to_string(),
		],
	)
	.await
	else {
		return Basis::unavailable(format!(
			"btrfs could not list what changed since generation {generation}"
		));
	};

	let paths = parse_find_new(&output, rel);
	debug!(
		generation,
		subvolume = %subvol.display(),
		changed = paths.len(),
		"btrfs listed what changed since the capture",
	);
	Basis::Named {
		paths,
		how: "btrfs's own record of what changed since the capture",
	}
}

/// The root of the subvolume `path` lives in, and its UUID — but only when the
/// mount exposes exactly that subvolume.
///
/// `findmnt` gives the mount point, which is not always the subvolume root: a
/// filesystem mounted at its top level (`subvolid=5`) with the cluster in a
/// nested subvolume has a mount point several levels above, and belonging to a
/// different subvolume. `find-new` run against the wrong subvolume does not
/// fail — it exits cleanly having found nothing, because the nested subvolume
/// keeps its own tree — and an empty answer taken as authoritative leaves every
/// diverged file in place. So the two are required to be the same subvolume, and
/// anything else declines.
async fn enclosing_subvolume(live: &Path) -> Option<(std::path::PathBuf, String)> {
	let mount = std::path::PathBuf::from(blockdev::findmnt("TARGET", live).await?);
	let (Some(at_mount), Some(at_live)) = (rootid(&mount).await, rootid(live).await) else {
		return None;
	};
	if at_mount != at_live {
		debug!(
			mount = %mount.display(),
			live = %live.display(),
			at_mount,
			at_live,
			"the mount point is not the live tree's own subvolume, so btrfs cannot be asked",
		);
		return None;
	}
	let uuid = subvolume_uuid(&mount).await?;
	Some((mount, uuid))
}

/// The UUID of the subvolume rooted at `path`.
///
/// Parsed by the same function the capture side records it with: the two have to
/// agree forever, and a divergence would turn the identity guard into a
/// permanent refusal, or worse a wrong match.
async fn subvolume_uuid(path: &Path) -> Option<String> {
	let out = blockdev::capture(
		"btrfs",
		&["subvolume", "show", "--", &path.to_string_lossy()],
	)
	.await?;
	crate::actions::canopy::backup::hold::parse_subvolume_uuid(&out)
}

/// The id of the subvolume a path belongs to.
async fn rootid(path: &Path) -> Option<u64> {
	blockdev::capture(
		"btrfs",
		&["inspect-internal", "rootid", "--", &path.to_string_lossy()],
	)
	.await?
	.trim()
	.parse()
	.ok()
}

/// The changed paths from `find-new`'s output, re-based from the subvolume root
/// onto the tree being restored.
///
/// A file line is a fixed set of fields ending `flags <FLAGS> <path>`, and the
/// path is everything after the flags — not the last whitespace-separated field,
/// which would truncate any name containing a space to its final word and then
/// silently drop it from an authoritative set. The closing `transid marker was
/// N` line is not a file. Anything outside `rel` is on the subvolume but not in
/// the tree, so it is not this restore's business.
fn parse_find_new(output: &str, rel: &Path) -> BTreeSet<std::path::PathBuf> {
	// Deduplicated while still borrowed: `find-new` emits one line per extent, so
	// a single large relation appears thousands of times and allocating a path per
	// line would be proportional to extents rather than to changed files.
	let unique: BTreeSet<&Path> = output
		.lines()
		.filter(|line| line.starts_with("inode "))
		.filter_map(|line| line.split_once(" flags "))
		.filter_map(|(_, after)| after.split_once(' '))
		.map(|(_flags, path)| Path::new(path))
		.filter_map(|path| path.strip_prefix(rel).ok())
		.collect();
	unique.into_iter().map(Path::to_path_buf).collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::PathBuf;

	/// A representative `find-new` run: two changed files under the cluster, one
	/// elsewhere on the subvolume, and the closing marker.
	const SAMPLE: &str = "\
inode 257 file offset 0 len 8192 disk start 13631488 offset 0 gen 42 flags NONE 16/main/base/1/2345
inode 258 file offset 0 len 4096 disk start 13639680 offset 0 gen 43 flags NONE 16/main/global/pg_control
inode 999 file offset 0 len 4096 disk start 13647872 offset 0 gen 44 flags NONE other/thing
transid marker was 41
";

	#[test]
	fn reads_the_changed_paths_relative_to_the_restored_tree() {
		let paths = parse_find_new(SAMPLE, Path::new("16/main"));
		assert!(paths.contains(&PathBuf::from("base/1/2345")));
		assert!(paths.contains(&PathBuf::from("global/pg_control")));
	}

	#[test]
	fn leaves_out_what_is_on_the_subvolume_but_not_in_the_tree() {
		let paths = parse_find_new(SAMPLE, Path::new("16/main"));
		assert_eq!(paths.len(), 2, "other/thing is not part of the restore");
	}

	#[test]
	fn the_closing_marker_is_not_a_file() {
		let paths = parse_find_new(SAMPLE, Path::new(""));
		assert!(
			!paths.iter().any(|p| p.to_string_lossy().contains("marker")),
			"got: {paths:?}"
		);
	}

	#[test]
	fn a_subvolume_that_is_the_tree_needs_no_rebasing() {
		let paths = parse_find_new(SAMPLE, Path::new(""));
		assert!(paths.contains(&PathBuf::from("16/main/base/1/2345")));
		assert!(paths.contains(&PathBuf::from("other/thing")));
	}

	#[test]
	fn nothing_changed_is_an_empty_set_not_an_absent_one() {
		// The difference matters: an empty set means "copy nothing", where an
		// absent basis means "go and compare".
		let paths = parse_find_new("transid marker was 41\n", Path::new(""));
		assert!(paths.is_empty());
	}

	/// Taking the last whitespace-separated field truncates a name with a space
	/// in it to its final word, which then fails to re-base and drops silently
	/// out of a set that is treated as authoritative — leaving the file diverged.
	#[test]
	fn a_path_containing_spaces_survives_intact() {
		let line = "inode 260 file offset 0 len 4096 disk start 1 offset 0 gen 45 flags NONE \
		            16/main/base/my table data\n";
		let paths = parse_find_new(line, Path::new("16/main"));
		assert_eq!(
			paths.into_iter().collect::<Vec<_>>(),
			vec![PathBuf::from("base/my table data")]
		);
	}

	#[test]
	fn a_path_containing_the_word_flags_is_not_cut_short() {
		let line = "inode 261 file offset 0 len 4096 disk start 1 offset 0 gen 46 flags NONE \
		            16/main/base/flags\n";
		let paths = parse_find_new(line, Path::new("16/main"));
		assert_eq!(
			paths.into_iter().collect::<Vec<_>>(),
			vec![PathBuf::from("base/flags")]
		);
	}
}
