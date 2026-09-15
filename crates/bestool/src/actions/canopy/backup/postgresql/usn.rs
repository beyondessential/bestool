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
//! (which gives it a new id and restarts its numbering), are both detected here
//! and yield nothing rather than a subset. The caller then compares the trees.
//!
//! Read through `fsutil`, which ships with Windows and is how the rest of this
//! module talks to the platform. The alternative is `FSCTL_READ_USN_JOURNAL`
//! directly, which this workspace cannot do: it forbids unsafe code.
//!
//! Every parse here is best-effort by design. `fsutil`'s output is human-facing
//! and localised, so an unrecognised shape yields `None` and the restore falls
//! back to comparing the trees — slower, never wrong. Verify on a real host
//! before relying on this as more than an optimisation.

use std::{
	collections::{BTreeSet, HashMap},
	path::{Path, PathBuf},
};

use tokio::process::Command;
use tracing::{debug, warn};

/// Where a volume's journal stood at a moment, enough to tell later whether it
/// still covers that moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
	/// The journal's identity. A journal deleted and recreated gets a new one,
	/// and its numbering restarts, so a position recorded against the old one
	/// means nothing against the new.
	pub journal_id: u64,
	/// The sequence number the next record will be written at.
	pub usn: u64,
}

/// Where the volume's journal stands now.
///
/// Called when a capture freezes, so a later restore has something to diff
/// against. `None` where the volume has no active journal, which is a supported
/// state rather than an error: a later restore compares the trees itself.
///
/// Only a shadow copy records a position, so only Windows calls this; the
/// parsing beneath it is exercised everywhere.
#[cfg(windows)]
pub async fn position(volume: &str) -> Option<Position> {
	let output = fsutil(&["usn", "queryjournal", &drive(volume)]).await?;
	let position = Position {
		journal_id: field(&output, "Usn Journal ID")?,
		usn: field(&output, "Next Usn")?,
	};
	debug!(volume, ?position, "read the change journal's position");
	Some(position)
}

/// The paths on `volume` that changed since `since`, relative to `root`.
///
/// `None` means the journal cannot answer — it is inactive, it has been
/// recreated, it has wrapped past the recorded position, or it could not be
/// read. The caller must then compare the trees rather than read an empty set as
/// "nothing changed": those are opposite conclusions.
pub async fn changed_since(volume: &str, since: Position, root: &Path) -> Option<BTreeSet<PathBuf>> {
	let drive = drive(volume);
	let current = fsutil(&["usn", "queryjournal", &drive]).await?;

	let journal_id: u64 = field(&current, "Usn Journal ID")?;
	if journal_id != since.journal_id {
		warn!(
			"volume {volume}'s change journal has been recreated since the capture, \
			 so it no longer says what changed"
		);
		return None;
	}
	// The ring has overwritten the records from the capture onwards, so what is
	// left is a suffix of the changes — and a suffix is not the set.
	let first: u64 = field(&current, "First Usn")?;
	if since.usn < first {
		warn!(
			"volume {volume}'s change journal has wrapped past the capture, so it no \
			 longer holds every change since"
		);
		return None;
	}

	let records = fsutil(&[
		"usn",
		"readjournal",
		&drive,
		&format!("startusn={:#x}", since.usn),
	])
	.await?;

	let mut paths = BTreeSet::new();
	let mut directories: HashMap<u64, Option<PathBuf>> = HashMap::new();
	for (name, parent) in parse_records(&records) {
		let Some(directory) = resolve_parent(&drive, parent, &mut directories).await else {
			// A directory that has since been deleted cannot be resolved, and does
			// not need to be: the walk finds a deletion structurally.
			continue;
		};
		if let Ok(rel) = directory.join(name).strip_prefix(root) {
			paths.insert(rel.to_path_buf());
		}
	}

	debug!(
		volume,
		changed = paths.len(),
		"the change journal named what diverged from the capture"
	);
	Some(paths)
}

/// The drive `fsutil` wants: a bare prefix such as `C:`.
fn drive(volume: &str) -> String {
	volume.trim_end_matches(['\\', '/']).to_owned()
}

async fn fsutil(args: &[&str]) -> Option<String> {
	let output = Command::new("fsutil")
		.args(args)
		.stdin(std::process::Stdio::null())
		.output()
		.await
		.inspect_err(|err| debug!("could not run fsutil {}: {err}", args.join(" ")))
		.ok()?;
	if !output.status.success() {
		debug!("fsutil {} exited {}", args.join(" "), output.status);
		return None;
	}
	Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A `Label : value` field from `fsutil`'s output, as a number. Values are
/// printed in hex with an `0x` prefix, decimal without one.
fn field(output: &str, label: &str) -> Option<u64> {
	let line = output
		.lines()
		.find(|line| line.trim_start().to_ascii_lowercase().starts_with(&label.to_ascii_lowercase()))?;
	parse_number(line.split_once(':')?.1)
}

/// A number as `fsutil` prints one, hex or decimal.
fn parse_number(text: &str) -> Option<u64> {
	let text = text.trim();
	match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
		Some(hex) => u64::from_str_radix(hex.trim(), 16).ok(),
		None => text.parse().ok(),
	}
}

/// The `(file name, parent directory id)` of every record in a
/// `fsutil usn readjournal` dump.
///
/// Records are blocks of `Label : value` lines. Only the name and the parent
/// matter: the reason a file changed is not interesting, because the question is
/// only whether it might differ, and every reason means it might.
fn parse_records(output: &str) -> Vec<(String, u64)> {
	let mut records = Vec::new();
	let mut name: Option<String> = None;
	let mut parent: Option<u64> = None;
	for line in output.lines() {
		let Some((label, value)) = line.split_once(':') else {
			continue;
		};
		let label = label.trim().to_ascii_lowercase();
		let value = value.trim();
		match label.as_str() {
			"file name" => {
				// A new name closes the record before it, since the name is the first
				// field of each block after the USN.
				if let (Some(name), Some(parent)) = (name.take(), parent.take()) {
					records.push((name, parent));
				}
				name = Some(value.to_owned());
			}
			"parent file id" => parent = parse_file_id(value),
			_ => {}
		}
	}
	if let (Some(name), Some(parent)) = (name, parent) {
		records.push((name, parent));
	}
	records
}

/// A file id as the journal prints one: 32 hex digits, of which NTFS uses the
/// low 64 bits as the file reference number.
fn parse_file_id(text: &str) -> Option<u64> {
	let text = text.trim().trim_start_matches("0x");
	if text.is_empty() || !text.chars().all(|c| c.is_ascii_hexdigit()) {
		return None;
	}
	let low = &text[text.len().saturating_sub(16)..];
	u64::from_str_radix(low, 16).ok()
}

/// The directory a file id names, cached: a journal covering hours of writes
/// names the same handful of directories over and over, and each lookup is a
/// process.
async fn resolve_parent(
	drive: &str,
	id: u64,
	cache: &mut HashMap<u64, Option<PathBuf>>,
) -> Option<PathBuf> {
	if let Some(cached) = cache.get(&id) {
		return cached.clone();
	}
	let resolved = query_name_by_id(drive, id).await;
	cache.insert(id, resolved.clone());
	resolved
}

async fn query_name_by_id(drive: &str, id: u64) -> Option<PathBuf> {
	let output = fsutil(&[
		"file",
		"queryfilenamebyid",
		&format!("{drive}\\"),
		&format!("{id:#018x}"),
	])
	.await?;
	parse_queried_name(&output, drive)
}

/// The path out of `fsutil file queryfilenamebyid`, which prints it in brackets
/// in extended-length form. The rest of the restore works in ordinary paths, and
/// the two forms do not compare equal.
fn parse_queried_name(output: &str, drive: &str) -> Option<PathBuf> {
	let start = output.find('[')? + 1;
	let rest = &output[start..];
	let end = rest.find(']')?;
	let path = rest[..end].trim();
	let path = path
		.strip_prefix(r"\\?\")
		.or_else(|| path.strip_prefix(r"\??\"))
		.unwrap_or(path);
	// A path on another volume cannot be part of this capture.
	path.to_ascii_uppercase()
		.starts_with(&drive.to_ascii_uppercase())
		.then(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
	use super::*;

	const QUERY: &str = "\
Usn Journal ID   : 0x01d5f4e2c3b1a098
First Usn        : 0x0000000000010000
Next Usn         : 0x0000000000123456
Lowest Valid Usn : 0x0000000000010000
Max Usn          : 0x00000fffffff0000
Maximum Size     : 0x0000000002000000
Allocation Delta : 0x0000000000400000
";

	const RECORDS: &str = "\
Usn                      : 0x0000000000010020
File name                : 2345
File name length         : 8
Reason                   : 0x00000002: Data extend
Time stamp               : 9/15/2026 5:26:03
Source info              : 0x00000000: *NONE*
Security Id              : 0x00000000
File attributes          : 0x00000020: Archive
File ID                  : 00000000000000000000000000012345
Parent file ID           : 00000000000000000000000000000041

Usn                      : 0x0000000000010080
File name                : pg_control
File name length         : 20
Reason                   : 0x80000002: Data extend | Close
Time stamp               : 9/15/2026 5:26:09
Source info              : 0x00000000: *NONE*
Security Id              : 0x00000000
File attributes          : 0x00000020: Archive
File ID                  : 00000000000000000000000000012346
Parent file ID           : 00000000000000000000000000000042
";

	#[test]
	fn reads_the_journals_identity_and_position() {
		assert_eq!(field(QUERY, "Usn Journal ID"), Some(0x01d5_f4e2_c3b1_a098));
		assert_eq!(field(QUERY, "Next Usn"), Some(0x0012_3456));
		assert_eq!(field(QUERY, "First Usn"), Some(0x0001_0000));
	}

	#[test]
	fn a_capture_before_the_first_record_has_been_wrapped_past() {
		// The check the ring makes necessary: a position older than what the
		// journal still holds means the answer would be a suffix, not the set.
		let first = field(QUERY, "First Usn").unwrap();
		assert!(0x0000_1000 < first, "a position the ring has discarded");
		assert!(0x0001_0000 >= first, "a position the ring still covers");
	}

	#[test]
	fn reads_every_record_as_a_name_under_a_directory() {
		let records = parse_records(RECORDS);
		assert_eq!(
			records,
			vec![("2345".to_owned(), 0x41), ("pg_control".to_owned(), 0x42)]
		);
	}

	#[test]
	fn an_empty_journal_has_no_records_rather_than_a_malformed_one() {
		assert!(parse_records("").is_empty());
	}

	#[test]
	fn takes_the_low_bits_of_a_128_bit_file_id() {
		assert_eq!(
			parse_file_id("00000000000000000000000000012345"),
			Some(0x12345)
		);
		assert_eq!(parse_file_id("0x2a"), Some(0x2a));
		assert_eq!(parse_file_id("not an id"), None);
	}

	#[test]
	fn reads_the_path_out_of_a_file_id_query() {
		let output = "A random link name to this file is [\\\\?\\C:\\Program Files\\PostgreSQL\\16\\data\\base\\1]\r\n";
		assert_eq!(
			parse_queried_name(output, "C:"),
			Some(PathBuf::from(r"C:\Program Files\PostgreSQL\16\data\base\1"))
		);
	}

	#[test]
	fn a_path_on_another_volume_is_not_part_of_this_capture() {
		let output = "A random link name to this file is [\\\\?\\D:\\elsewhere]\r\n";
		assert_eq!(parse_queried_name(output, "C:"), None);
	}

	#[test]
	fn unrecognised_output_yields_nothing_rather_than_a_wrong_path() {
		assert_eq!(parse_queried_name("Error: Access is denied.", "C:"), None);
		assert_eq!(field("Error: Access is denied.", "Next Usn"), None);
	}

	#[test]
	fn reads_both_the_hex_and_decimal_forms_fsutil_prints() {
		assert_eq!(parse_number(" 0x0000000000123456 "), Some(0x0012_3456));
		assert_eq!(parse_number(" 1193046 "), Some(1_193_046));
		assert_eq!(parse_number(" UNKNOWN "), None);
	}

	#[test]
	fn a_drive_is_taken_without_its_trailing_separator() {
		assert_eq!(drive("C:\\"), "C:");
		assert_eq!(drive("C:"), "C:");
	}
}
