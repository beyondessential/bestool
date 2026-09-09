//! Where the audit log lives and how its files are named.
//!
//! spec: AUD-STO, AUD-RET

use std::path::{Path, PathBuf};

use jiff::civil::Date;
use miette::{IntoDiagnostic, Result, WrapErr as _, miette};
use uuid::Uuid;

/// Extension of a plain segment: the framing, not a format a JSON-lines reader
/// would cope with.
pub const SEGMENT_EXT: &str = ".json-seq";

/// Extension of a compacted day file.
pub const DAY_FILE_EXT: &str = ".json-seq.zst";

/// Every audit file is named for the day it covers, so sorting names orders the
/// log by time.
const PREFIX: &str = "audit-";

/// Lock file taken for the duration of compaction, retention and legacy import.
pub const DIRECTORY_LOCK: &str = "audit.lock";

/// Suffix of the file a segment's lock is taken on.
///
/// The lock is not taken on the segment itself. A lock over a file's own bytes
/// is enforced rather than advisory on Windows, where an exclusive one would
/// stop every reader of a live segment and a shared one would stop the writer's
/// own appends. A file that holds nothing and that nobody reads has neither
/// problem, and gives the same answer to the only question being asked: is a
/// session writing this segment?
pub const LOCK_EXT: &str = ".lock";

/// Suffix of the temporary name a day file is written under before it is
/// renamed into place.
pub const TEMP_SUFFIX: &str = ".tmp";

/// Legacy single-file store names, recognised on import.
pub const LEGACY_NAMES: &[&str] = &["audit-main.redb", "history.redb"];

/// Legacy working and orphaned copy prefixes, recognised on import.
pub const LEGACY_PREFIXES: &[&str] = &["audit-working-", "audit-orphaned-"];

/// Suffix put on a legacy file once its records have been imported.
///
/// It stops the file being imported again, and leaves the original where an
/// auditor can still compare against it. The date it was set aside on goes in
/// the name so that retention can take it in its turn: what it holds is the
/// same statement text as the log itself, and is kept no longer.
pub const IMPORTED_SUFFIX: &str = ".imported";

/// Suffix of a segment being written by an import that has not finished.
pub const PART_SUFFIX: &str = ".part";

/// The name an imported legacy file is set aside under.
pub fn set_aside(legacy: &Path, on: Date) -> PathBuf {
	let mut name = legacy.as_os_str().to_os_string();
	name.push(format!(".{on}{IMPORTED_SUFFIX}"));
	PathBuf::from(name)
}

/// The name a segment is written under while its import is unfinished.
pub fn part_of(segment: &Path) -> PathBuf {
	let mut name = segment.as_os_str().to_os_string();
	name.push(PART_SUFFIX);
	PathBuf::from(name)
}

/// Legacy files set aside by an import, with the day each was set aside on.
pub fn list_set_aside(dir: &Path) -> Result<Vec<(PathBuf, Date)>> {
	let mut found = Vec::new();

	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(found),
		Err(err) => return Err(err).into_diagnostic(),
	};

	for entry in entries {
		let entry = entry.into_diagnostic()?;
		let name = entry.file_name();
		let Some(name) = name.to_str() else { continue };
		let Some(rest) = name.strip_suffix(IMPORTED_SUFFIX) else {
			continue;
		};
		// `<original>.<YYYY-MM-DD>`: the date is fixed-width and last.
		let Some(date) = rest.len().checked_sub(10).and_then(|at| rest.get(at..)) else {
			continue;
		};
		if let Ok(date) = date.parse() {
			found.push((entry.path(), date));
		}
	}

	found.sort();
	Ok(found)
}

/// Name of the segment a session writes on a given day.
pub fn segment_name(date: Date, instance: Uuid) -> String {
	format!("{PREFIX}{date}-{instance}{SEGMENT_EXT}")
}

/// Name of the day file covering a given day.
pub fn day_file_name(date: Date) -> String {
	format!("{PREFIX}{date}{DAY_FILE_EXT}")
}

/// The file a segment's lock is taken on.
pub fn lock_of(segment: &Path) -> PathBuf {
	let mut name = segment.as_os_str().to_os_string();
	name.push(LOCK_EXT);
	PathBuf::from(name)
}

/// Segment lock files in a directory, with the segment each belongs to.
pub fn list_locks(dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
	let mut found = Vec::new();

	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(found),
		Err(err) => return Err(err).into_diagnostic(),
	};

	for entry in entries {
		let entry = entry.into_diagnostic()?;
		let name = entry.file_name();
		let Some(name) = name.to_str() else { continue };
		let Some(segment) = name.strip_suffix(LOCK_EXT) else {
			continue;
		};
		if classify(segment).is_some_and(|kind| kind.instance().is_some()) {
			found.push((entry.path(), dir.join(segment)));
		}
	}

	found.sort();
	Ok(found)
}

/// What a file in the audit directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditFile {
	Segment { date: Date, instance: Uuid },
	DayFile { date: Date },
}

impl AuditFile {
	pub fn date(&self) -> Date {
		match self {
			Self::Segment { date, .. } | Self::DayFile { date } => *date,
		}
	}

	pub fn instance(&self) -> Option<Uuid> {
		match self {
			Self::Segment { instance, .. } => Some(*instance),
			Self::DayFile { .. } => None,
		}
	}
}

/// Classify a file name, or `None` when it is not part of the log.
pub fn classify(name: &str) -> Option<AuditFile> {
	let rest = name.strip_prefix(PREFIX)?;

	if let Some(stem) = rest.strip_suffix(DAY_FILE_EXT) {
		return Some(AuditFile::DayFile {
			date: stem.parse().ok()?,
		});
	}

	let stem = rest.strip_suffix(SEGMENT_EXT)?;
	// `YYYY-MM-DD-<uuid>`: the date is fixed-width, so split at its end rather
	// than on a separator the UUID also contains.
	let (date, instance) = stem.split_at_checked(10)?;
	Some(AuditFile::Segment {
		date: date.parse().ok()?,
		instance: instance.strip_prefix('-')?.parse().ok()?,
	})
}

/// Whether a file name belongs to a store in the earlier single-file format.
pub fn is_legacy(name: &str) -> bool {
	LEGACY_NAMES.contains(&name)
		|| (LEGACY_PREFIXES.iter().any(|p| name.starts_with(p)) && name.ends_with(".redb"))
}

/// Every audit file in a directory, oldest day first.
///
/// Names that belong to neither the current nor the legacy format are ignored,
/// so an operator's own notes in the directory do not break a read.
pub fn list(dir: &Path) -> Result<Vec<(PathBuf, AuditFile)>> {
	let mut found = Vec::new();

	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(found),
		Err(err) => return Err(err).into_diagnostic(),
	};

	for entry in entries {
		let entry = entry.into_diagnostic()?;
		let name = entry.file_name();
		let Some(name) = name.to_str() else { continue };
		if let Some(kind) = classify(name) {
			found.push((entry.path(), kind));
		}
	}

	// Day files sort after the segments they may supersede, so a reader that
	// meets a duplicated record has already seen the segment's copy.
	found.sort_by_key(|(path, kind)| (kind.date(), kind.instance().is_none(), path.clone()));
	Ok(found)
}

/// Legacy store files in a directory.
pub fn list_legacy(dir: &Path) -> Result<Vec<PathBuf>> {
	let mut found = Vec::new();

	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(found),
		Err(err) => return Err(err).into_diagnostic(),
	};

	for entry in entries {
		let entry = entry.into_diagnostic()?;
		let name = entry.file_name();
		if name.to_str().is_some_and(is_legacy) {
			found.push(entry.path());
		}
	}

	found.sort();
	Ok(found)
}

/// The per-user state directory the log defaults to.
pub fn default_dir() -> Result<PathBuf> {
	let dir = platform_default_dir()?;
	create_dir(&dir)?;
	Ok(dir)
}

/// Create the audit directory, readable by its owner alone.
///
/// The directory belongs to one operating-system user, and what it holds is the
/// full text of every statement run against the database, which for a clinical
/// deployment means patient data in plain, greppable JSON. Other local users
/// have no business reading it.
pub fn create_dir(dir: &Path) -> Result<()> {
	// Created private, rather than created and then narrowed: between the two
	// another local user could open a directory that is about to hold patient
	// data and keep reading it afterwards.
	let mut builder = std::fs::DirBuilder::new();
	builder.recursive(true);

	#[cfg(unix)]
	{
		use std::os::unix::fs::DirBuilderExt as _;
		builder.mode(0o700);
	}

	builder
		.create(dir)
		.into_diagnostic()
		.wrap_err_with(|| format!("creating audit directory {}", dir.display()))?;

	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;
		// A directory that already existed with wider permissions is narrowed
		// too. A failure must not stop a session recording, but it does mean
		// patient data is about to be written where other local users can read
		// it, so it is said out loud rather than swallowed.
		if let Err(err) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
			tracing::warn!(?err, ?dir, "cannot narrow the audit directory");
			eprintln!(
				"warning: the audit directory {} could not be made private: {err}",
				dir.display()
			);
			eprintln!("warning: other users of this machine may be able to read the audit log");
		}
	}

	Ok(())
}

/// Open options for a file in the audit directory, readable by its owner alone.
pub fn private() -> std::fs::OpenOptions {
	let mut options = std::fs::OpenOptions::new();

	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt as _;
		options.mode(0o600);
	}

	options
}

/// The default directory for help text, which must not fail.
pub fn default_dir_for_help() -> String {
	if let Ok(path) = platform_default_dir() {
		return path.display().to_string();
	}

	#[cfg(target_os = "macos")]
	{
		"~/Library/Application Support/bestool-psql".into()
	}
	#[cfg(target_os = "windows")]
	{
		"%LOCALAPPDATA%\\bestool-psql".into()
	}
	#[cfg(not(any(target_os = "macos", target_os = "windows")))]
	{
		"~/.local/state/bestool-psql".into()
	}
}

fn platform_default_dir() -> Result<PathBuf> {
	#[cfg(not(any(target_os = "macos", target_os = "windows")))]
	{
		if let Some(dir) = dirs::state_dir() {
			Ok(dir.join("bestool-psql"))
		} else if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
			Ok(PathBuf::from(dir).join("bestool-psql"))
		} else if let Some(home) = std::env::var_os("HOME") {
			Ok(PathBuf::from(home)
				.join(".local")
				.join("state")
				.join("bestool-psql"))
		} else {
			Err(miette!("could not determine home directory"))
		}
	}

	#[cfg(any(target_os = "macos", target_os = "windows"))]
	{
		if let Some(dir) = dirs::data_local_dir() {
			return Ok(dir.join("bestool-psql"));
		}

		#[cfg(target_os = "macos")]
		{
			if let Some(home) = std::env::var_os("HOME") {
				Ok(PathBuf::from(home)
					.join("Library")
					.join("Application Support")
					.join("bestool-psql"))
			} else {
				Err(miette!("could not determine home directory"))
			}
		}
		#[cfg(target_os = "windows")]
		{
			if let Some(localappdata) = std::env::var_os("LOCALAPPDATA") {
				Ok(PathBuf::from(localappdata).join("bestool-psql"))
			} else {
				Err(miette!("could not determine LOCALAPPDATA directory"))
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const UUID: &str = "7d2c0f4e-1b3a-4c5d-8e9f-0a1b2c3d4e5f";

	fn date(s: &str) -> Date {
		s.parse().unwrap()
	}

	#[test]
	fn segment_names_round_trip() {
		let instance: Uuid = UUID.parse().unwrap();
		let name = segment_name(date("2026-09-08"), instance);
		assert_eq!(name, format!("audit-2026-09-08-{UUID}.json-seq"));
		assert_eq!(
			classify(&name),
			Some(AuditFile::Segment {
				date: date("2026-09-08"),
				instance
			})
		);
	}

	#[test]
	fn day_file_names_round_trip() {
		let name = day_file_name(date("2026-09-08"));
		assert_eq!(name, "audit-2026-09-08.json-seq.zst");
		assert_eq!(
			classify(&name),
			Some(AuditFile::DayFile {
				date: date("2026-09-08")
			})
		);
	}

	#[test]
	fn unrelated_names_are_not_audit_files() {
		for name in [
			DIRECTORY_LOCK,
			"audit-2026-09-08-7d2c0f4e-1b3a-4c5d-8e9f-0a1b2c3d4e5f.json-seq.lock",
			"audit-main.redb",
			"audit-working-7d2c.redb",
			"notes.txt",
			"audit-2026-09-08.json-seq.zst.tmp",
			"audit-not-a-date-7d2c0f4e-1b3a-4c5d-8e9f-0a1b2c3d4e5f.json-seq",
			"audit-2026-09-08-not-a-uuid.json-seq",
		] {
			assert_eq!(classify(name), None, "{name} should not classify");
		}
	}

	#[test]
	fn legacy_names_are_recognised() {
		for name in [
			"audit-main.redb",
			"history.redb",
			"audit-working-7d2c.redb",
			"audit-orphaned-7d2c.redb",
		] {
			assert!(is_legacy(name), "{name} should be legacy");
		}
		for name in ["audit-2026-09-08.json-seq.zst", "notes.redb.txt"] {
			assert!(!is_legacy(name), "{name} should not be legacy");
		}
	}

	#[test]
	fn listing_orders_by_day_and_puts_day_files_last() {
		let dir = tempfile::tempdir().unwrap();
		let a: Uuid = UUID.parse().unwrap();
		for name in [
			day_file_name(date("2026-09-09")),
			segment_name(date("2026-09-09"), a),
			segment_name(date("2026-09-07"), a),
			DIRECTORY_LOCK.to_string(),
		] {
			std::fs::write(dir.path().join(name), b"").unwrap();
		}

		let found = list(dir.path()).unwrap();
		let kinds: Vec<_> = found.iter().map(|(_, kind)| *kind).collect();
		assert_eq!(
			kinds,
			vec![
				AuditFile::Segment {
					date: date("2026-09-07"),
					instance: a
				},
				AuditFile::Segment {
					date: date("2026-09-09"),
					instance: a
				},
				AuditFile::DayFile {
					date: date("2026-09-09")
				},
			]
		);
	}
}
