//! Free space, sizes, and how to say them.
//!
//! Asked by the backup path before it stages a copy and by the restore path
//! before it writes a divergence, so it belongs to neither. Keeping it here is
//! what stops the generic restore machinery reaching into one backup method's
//! internals for a question that has nothing to do with postgres.

use std::path::Path;

/// Bytes free on the volume backing `path`, statting the nearest existing
/// ancestor (the staging root itself usually doesn't exist yet). `None` if it
/// can't be determined — a stat failure must never block a backup.
pub fn available(path: &Path) -> Option<u64> {
	let mut current = Some(path);
	while let Some(p) = current {
		if p.exists() {
			return fs4::available_space(p).ok();
		}
		current = p.parent();
	}
	None
}

/// Total on-disk size of the files under `root`, following no symlinks (external
/// tablespaces are covered by the SQL estimate instead). Best-effort: unreadable
/// entries are skipped. Returns 0 if nothing could be read.
pub async fn dir_size(root: &Path) -> u64 {
	let root = root.to_path_buf();
	tokio::task::spawn_blocking(move || walk_size(&root))
		.await
		.unwrap_or(0)
}

fn walk_size(root: &Path) -> u64 {
	let mut total = 0u64;
	let mut stack = vec![root.to_path_buf()];
	while let Some(dir) = stack.pop() {
		let Ok(entries) = std::fs::read_dir(&dir) else {
			continue;
		};
		for entry in entries.flatten() {
			let Ok(file_type) = entry.file_type() else {
				continue;
			};
			if file_type.is_dir() {
				stack.push(entry.path());
			} else if file_type.is_file()
				&& let Ok(meta) = entry.metadata()
			{
				total = total.saturating_add(meta.len());
			}
		}
	}
	total
}

/// Format a byte count as a human-readable size (binary units).
pub fn fmt_bytes(bytes: u64) -> String {
	const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
	let mut value = bytes as f64;
	let mut unit = 0;
	while value >= 1024.0 && unit < UNITS.len() - 1 {
		value /= 1024.0;
		unit += 1;
	}
	if unit == 0 {
		format!("{bytes} B")
	} else {
		format!("{value:.1} {}", UNITS[unit])
	}
}


#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn fmt_bytes_scales_units() {
		assert_eq!(fmt_bytes(512), "512 B");
		assert_eq!(fmt_bytes(1024), "1.0 KiB");
		assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MiB");
		assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
	}

	#[tokio::test]
	async fn dir_size_sums_files_recursively() {
		let tmp = tempfile::tempdir().unwrap();
		std::fs::write(tmp.path().join("a"), vec![0u8; 100]).unwrap();
		let sub = tmp.path().join("sub");
		std::fs::create_dir(&sub).unwrap();
		std::fs::write(sub.join("b"), vec![0u8; 200]).unwrap();
		assert_eq!(dir_size(tmp.path()).await, 300);
	}

	#[test]
	fn available_walks_up_to_something_that_exists() {
		// The staging root usually does not exist yet, and free space is a property
		// of the volume rather than of a directory.
		let tmp = tempfile::tempdir().unwrap();
		assert!(available(&tmp.path().join("not/here/yet")).is_some());
	}
}
