//! Estimating a base backup's size and finding room for it.
//!
//! `pg_basebackup` stages a full copy of the cluster before kopia snapshots it,
//! so a run can exhaust a small system drive. This module estimates how much the
//! copy needs, checks free space, and — on Windows — steers the copy onto a
//! roomier disk (a `Backup`/`Backups` folder on another drive) when the default
//! location is too small. The checks run inside `prepare`, so a shortfall surfaces
//! as an ordinary backup failure reported to canopy.
//!
//! The pure helpers (headroom, candidate selection) are unit-tested; the live
//! `psql` size query, filesystem walk, and drive enumeration are verified on-host.

use std::path::{Path, PathBuf};

use miette::{Result, bail};

use super::super::method::PostgresqlConfig;

/// Headroom over the raw estimate: the larger of a fifth of it or 1 GiB. Covers
/// cluster-global files and the WAL streamed during the backup, plus slack.
const HEADROOM_DIVISOR: u64 = 5;
const HEADROOM_FLOOR: u64 = 1024 * 1024 * 1024;

/// Minimum free space required on the source volume before taking a VSS shadow.
/// The shadow's copy-on-write area grows with writes during the backup, not with
/// the database size, so this is a floor rather than a full-size reservation.
#[cfg(windows)]
const VSS_FLOOR: u64 = 1024 * 1024 * 1024;

/// Free space required to stage a base backup whose copy is about `need` bytes.
pub fn required_free(need: u64) -> u64 {
	need.saturating_add((need / HEADROOM_DIVISOR).max(HEADROOM_FLOOR))
}

/// Free space required on the source volume for a VSS shadow of an estimated
/// `need`-byte cluster (a tenth of it, floored at 1 GiB).
#[cfg(windows)]
pub fn vss_required_free(need: Option<u64>) -> u64 {
	need.map_or(VSS_FLOOR, |n| (n / 10).max(VSS_FLOOR))
}

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

/// Estimate the base backup's on-disk size: the larger of the server's reported
/// total database size and an on-disk walk of the data directory. `None` only if
/// both fail, in which case the caller proceeds without gating on space.
pub async fn estimate_needed(config: &PostgresqlConfig, data_dir: &Path) -> Option<u64> {
	let sql = db_size_sql(config).await;
	let walk = match dir_size(data_dir).await {
		0 => None,
		n => Some(n),
	};
	match (sql, walk) {
		(Some(a), Some(b)) => Some(a.max(b)),
		(a, b) => a.or(b),
	}
}

/// The server's total on-disk size across all databases (includes indexes,
/// bloat, and external tablespaces). `None` on any failure.
async fn db_size_sql(config: &PostgresqlConfig) -> Option<u64> {
	let mut cmd = super::pg_command(&super::postgres_bin("psql", &config_data_dir(config)));
	// -w: never block on a password prompt (matches the CHECKPOINT/verify queries).
	cmd.args(["-X", "-q", "-w", "-tAc"]);
	cmd.arg("SELECT COALESCE(sum(pg_database_size(oid)), 0)::bigint FROM pg_database");
	super::apply_connection(&mut cmd, config);
	cmd.stdin(std::process::Stdio::null());
	let output = cmd.output().await.ok()?;
	if !output.status.success() {
		return None;
	}
	String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// The data dir for locating `psql`; only used to find the binary, so an empty
/// path (falling back to `PATH`) is fine when the config doesn't override it.
fn config_data_dir(config: &PostgresqlConfig) -> PathBuf {
	config.data_dir.clone().unwrap_or_default()
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

/// A place the base backup could be staged, with the free space on its volume.
struct Candidate {
	/// Full staging root to use if this candidate is chosen.
	root: PathBuf,
	/// Free bytes on its volume, or `None` if it couldn't be determined.
	available: Option<u64>,
	/// A disk auto-detected by its `Backup`/`Backups` folder — preferred over the
	/// system default so the copy lands off the system drive.
	detected: bool,
}

/// The staging root under a base directory: `<base>/bestool/backup-source/<type>`,
/// matching the layout of the default location.
fn staging_root_under(base: &Path, backup_type: &str) -> PathBuf {
	base.join("bestool")
		.join("backup-source")
		.join(backup_type)
}

/// Choose where to stage the base backup.
///
/// An explicit `override_dir` is used as-is, failing early if it lacks room. With
/// no override, a detected `Backup`/`Backups` disk with enough space wins (the
/// roomiest), else the system default; if nothing has room, the run fails early
/// with each candidate's free space. When `need` is unknown, the space gate is
/// skipped and the default (or override) is used.
pub fn choose_staging_root(
	backup_type: &str,
	override_dir: Option<&Path>,
	need: Option<u64>,
) -> Result<PathBuf> {
	if let Some(dir) = override_dir {
		let root = staging_root_under(dir, backup_type);
		if let Some(need) = need {
			let free = available(&root);
			if free.is_some_and(|f| f < required_free(need)) {
				bail!(
					"configured staging directory {} has {} free, but the base backup needs about {} ({} with headroom); free space there or point staging at a larger disk",
					dir.display(),
					fmt_bytes(free.unwrap_or(0)),
					fmt_bytes(need),
					fmt_bytes(required_free(need)),
				);
			}
		}
		return Ok(root);
	}

	let default_root = super::stable_source_dir(backup_type);
	let Some(need) = need else {
		return Ok(default_root);
	};

	let mut candidates = vec![Candidate {
		available: available(&default_root),
		root: default_root,
		detected: false,
	}];
	for base in detected_backup_dirs() {
		let root = staging_root_under(&base, backup_type);
		candidates.push(Candidate {
			available: available(&root),
			root,
			detected: true,
		});
	}

	match pick(&candidates, required_free(need)) {
		Some(root) => Ok(root),
		None => bail!("{}", shortfall_message(&candidates, need)),
	}
}

/// Pick the staging root: a detected disk with room (the roomiest) if any, else
/// the system default if it has room, else `None`.
fn pick(candidates: &[Candidate], required: u64) -> Option<PathBuf> {
	let fits = |c: &&Candidate| c.available.is_some_and(|a| a >= required);
	candidates
		.iter()
		.filter(|c| c.detected)
		.filter(fits)
		.max_by_key(|c| c.available.unwrap_or(0))
		.or_else(|| candidates.iter().filter(|c| !c.detected).find(fits))
		.map(|c| c.root.clone())
}

/// The early-failure message when no candidate has room, naming each one's free
/// space so an operator knows where to make room or add a `Backup` folder.
fn shortfall_message(candidates: &[Candidate], need: u64) -> String {
	let mut msg = format!(
		"not enough disk space to stage the base backup: it needs about {} ({} with headroom), but",
		fmt_bytes(need),
		fmt_bytes(required_free(need)),
	);
	for candidate in candidates {
		msg.push_str(&format!(
			"\n  - {} has {}",
			candidate.root.display(),
			candidate
				.available
				.map_or_else(|| "unknown free space".to_owned(), fmt_bytes),
		));
	}
	msg
}

/// Fixed disks carrying a top-level `Backup`/`Backups` folder, where a base
/// backup could be staged. Windows-only (the drive-letter `Backup`-folder
/// convention doesn't apply elsewhere); empty on other platforms.
#[cfg(windows)]
fn detected_backup_dirs() -> Vec<PathBuf> {
	let disks = sysinfo::Disks::new_with_refreshed_list();
	let mut out = Vec::new();
	for disk in &disks {
		if disk.is_removable() {
			continue;
		}
		let mount = disk.mount_point();
		// Windows paths are case-insensitive, so a direct join matches any casing.
		for name in ["Backup", "Backups"] {
			let folder = mount.join(name);
			if folder.is_dir() {
				out.push(folder);
			}
		}
	}
	out
}

#[cfg(not(windows))]
fn detected_backup_dirs() -> Vec<PathBuf> {
	Vec::new()
}

/// Format a byte count as a human-readable size (binary units).
pub(super) fn fmt_bytes(bytes: u64) -> String {
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
	fn required_free_uses_the_larger_of_a_fifth_or_a_gib() {
		// Small DB: the 1 GiB floor dominates the 20% fraction.
		assert_eq!(required_free(100 * 1024 * 1024), 100 * 1024 * 1024 + HEADROOM_FLOOR);
		// Large DB: 20% exceeds the floor.
		let ten_gib = 10 * 1024 * 1024 * 1024;
		assert_eq!(required_free(ten_gib), ten_gib + ten_gib / 5);
	}

	#[cfg(windows)]
	#[test]
	fn vss_required_free_floors_at_a_gib() {
		assert_eq!(vss_required_free(None), VSS_FLOOR);
		assert_eq!(vss_required_free(Some(1024)), VSS_FLOOR);
		let hundred_gib = 100 * 1024 * 1024 * 1024;
		assert_eq!(vss_required_free(Some(hundred_gib)), hundred_gib / 10);
	}

	#[test]
	fn estimate_absent_when_both_unknown() {
		// (pure combination check; the async fn wires these together)
		let combine = |sql: Option<u64>, walk: Option<u64>| match (sql, walk) {
			(Some(a), Some(b)) => Some(a.max(b)),
			(a, b) => a.or(b),
		};
		assert_eq!(combine(None, None), None);
		assert_eq!(combine(Some(5), None), Some(5));
		assert_eq!(combine(None, Some(7)), Some(7));
		assert_eq!(combine(Some(5), Some(7)), Some(7));
	}

	fn candidate(root: &str, available: Option<u64>, detected: bool) -> Candidate {
		Candidate {
			root: PathBuf::from(root),
			available,
			detected,
		}
	}

	#[test]
	fn pick_prefers_the_roomiest_detected_disk_with_room() {
		let candidates = [
			candidate("/system", Some(100), false),
			candidate("/disk-d", Some(500), true),
			candidate("/disk-e", Some(800), true),
		];
		assert_eq!(pick(&candidates, 50), Some(PathBuf::from("/disk-e")));
	}

	#[test]
	fn pick_falls_back_to_system_when_no_detected_disk_fits() {
		let candidates = [
			candidate("/system", Some(1000), false),
			candidate("/disk-d", Some(10), true),
		];
		assert_eq!(pick(&candidates, 500), Some(PathBuf::from("/system")));
	}

	#[test]
	fn pick_none_when_nothing_fits() {
		let candidates = [
			candidate("/system", Some(10), false),
			candidate("/disk-d", Some(20), true),
			candidate("/disk-e", None, true),
		];
		assert_eq!(pick(&candidates, 500), None);
	}

	#[test]
	fn choose_uses_override_verbatim_when_it_fits() {
		// need = None skips the gate; the override root is returned regardless.
		let root = choose_staging_root("pg", Some(Path::new("/mnt/backups")), None).unwrap();
		assert_eq!(root, PathBuf::from("/mnt/backups/bestool/backup-source/pg"));
	}

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
}
