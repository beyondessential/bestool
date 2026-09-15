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

use tokio::process::Command;
use tracing::debug;

use super::Basis;

/// The paths under `live` written since `generation`.
pub async fn changed_since(generation: u64, live: &Path) -> Basis {
	// `find-new` works on a subvolume, and the live tree is usually a directory
	// within one, so its answers are relative to the subvolume root and have to
	// be re-based onto the tree being restored.
	let Some(subvol) = enclosing_subvolume(live).await else {
		return Basis::unavailable("the live tree is not on a btrfs subvolume this can read");
	};
	let Ok(rel) = live.strip_prefix(&subvol) else {
		return Basis::unavailable("the live tree is not under the subvolume btrfs reports");
	};

	let output = Command::new("btrfs")
		.args(["subvolume", "find-new"])
		.arg(&subvol)
		.arg(generation.to_string())
		.stdin(std::process::Stdio::null())
		.output()
		.await;
	let output = match output {
		Ok(output) if output.status.success() => output,
		Ok(output) => {
			return Basis::unavailable(format!(
				"btrfs could not list what changed since generation {generation} ({})",
				output.status
			));
		}
		Err(err) => {
			return Basis::unavailable(format!("btrfs could not be run to list what changed: {err}"));
		}
	};

	let paths = parse_find_new(&String::from_utf8_lossy(&output.stdout), rel);
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

/// The subvolume `path` lives in, as btrfs reports its mount.
async fn enclosing_subvolume(path: &Path) -> Option<std::path::PathBuf> {
	let output = Command::new("findmnt")
		.args(["-n", "-o", "TARGET", "--target"])
		.arg(path)
		.stdin(std::process::Stdio::null())
		.output()
		.await
		.ok()?;
	if !output.status.success() {
		return None;
	}
	let target = String::from_utf8_lossy(&output.stdout).trim().to_owned();
	(!target.is_empty()).then(|| std::path::PathBuf::from(target))
}

/// The changed paths from `find-new`'s output, re-based from the subvolume root
/// onto the tree being restored.
///
/// Each file line ends `… gen N … flags … <path>`, the path being the last
/// whitespace-separated field and relative to the subvolume root. The closing
/// `transid marker was N` line is not a file. Anything outside `rel` is on the
/// subvolume but not in the tree, so it is not the restore's business.
fn parse_find_new(output: &str, rel: &Path) -> BTreeSet<std::path::PathBuf> {
	output
		.lines()
		.filter(|line| line.starts_with("inode "))
		.filter_map(|line| line.rsplit_once(char::is_whitespace))
		.map(|(_, path)| Path::new(path))
		.filter_map(|path| path.strip_prefix(rel).ok())
		.map(Path::to_path_buf)
		.collect()
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
}
