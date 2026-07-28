//! Parse kopia's foreground `--progress` line into structured counters.
//!
//! kopia rewrites a single status line on stderr as a snapshot uploads, e.g.
//!
//! ```text
//!  | 3 hashing, 0 hashed (65.5 KB), 0 cached (0 B), uploaded 0 B, estimating...
//!  * 0 hashing, 3 hashed (140 MB), 0 cached (0 B), uploaded 137.5 MB, estimating...
//! ```
//!
//! and once its pre-scan estimate is ready the tail becomes `estimated <size>
//! (<pct>%) <duration> left`. The byte figures are base-10 human units, so they
//! are inherently rounded — a coarser resolution than the exact total kopia
//! reports in its final JSON.
//!
//! Only the leading counters are load-bearing for identifying a progress line;
//! the optional estimate and error figures in the tail are extracted leniently,
//! so a change in their ordering or wording does not stop the line parsing.

use winnow::{
	ModalResult, Parser,
	ascii::{digit1, float, space0, space1},
	token::{one_of, take_while},
};

/// Counters read from one kopia progress line. Byte figures are rounded (kopia
/// prints human units); counts are exact. Fields kopia does not put on the line
/// are left at their default / `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KopiaProgress {
	/// Files currently being hashed.
	pub hashing_files: i64,
	/// Files hashed (processed) so far.
	pub hashed_files: i64,
	/// Bytes hashed (processed) so far.
	pub hashed_bytes: i64,
	/// Files found already present in the repository.
	pub cached_files: i64,
	/// Bytes found already present in the repository.
	pub cached_bytes: i64,
	/// Bytes uploaded so far.
	pub uploaded_bytes: i64,
	/// Total bytes the run expects to handle, once kopia has estimated it.
	pub estimated_bytes: Option<i64>,
	/// Errors hit so far, when the line reports them.
	pub errors: Option<i64>,
	/// Errors hit and ignored so far, when the line reports them.
	pub ignored_errors: Option<i64>,
}

/// Parse a kopia progress line, or `None` if the line is not one (e.g. a
/// maintenance or snapshotting line).
pub fn parse_progress(line: &str) -> Option<KopiaProgress> {
	let mut input = line;
	let mut progress = leading_counters.parse_next(&mut input).ok()?;

	// The tail (estimate, errors) varies in wording and order across kopia
	// versions; pull each figure out of the remainder wherever it sits rather
	// than parsing the tail positionally.
	let rest = input;
	progress.estimated_bytes = bytes_after(rest, "estimated ");
	progress.errors = error_count(rest, "errors").or_else(|| error_count(rest, "error"));
	progress.ignored_errors = ignored_count(rest);
	Some(progress)
}

/// Parse the mandatory leading part: `<spinner> N hashing, N hashed (B), N
/// cached (B), uploaded B`. This both identifies the line as progress and reads
/// the core counters.
fn leading_counters(input: &mut &str) -> ModalResult<KopiaProgress> {
	space0.parse_next(input)?;
	// kopia cycles a spinner glyph at the head of the line.
	one_of(['|', '/', '-', '\\', '*']).parse_next(input)?;
	space1.parse_next(input)?;

	let hashing_files = integer.parse_next(input)?;
	" hashing, ".parse_next(input)?;
	let hashed_files = integer.parse_next(input)?;
	" hashed (".parse_next(input)?;
	let hashed_bytes = bytes.parse_next(input)?;
	"), ".parse_next(input)?;
	let cached_files = integer.parse_next(input)?;
	" cached (".parse_next(input)?;
	let cached_bytes = bytes.parse_next(input)?;
	"), uploaded ".parse_next(input)?;
	let uploaded_bytes = bytes.parse_next(input)?;

	Ok(KopiaProgress {
		hashing_files,
		hashed_files,
		hashed_bytes,
		cached_files,
		cached_bytes,
		uploaded_bytes,
		..Default::default()
	})
}

/// A base-10 integer.
fn integer(input: &mut &str) -> ModalResult<i64> {
	digit1.parse_to().parse_next(input)
}

/// A human byte size as kopia prints it (base-10 units), e.g. `0 B`, `65.5 KB`,
/// `137.5 MB`, `1.2 GB`. Rounded to the nearest byte.
fn bytes(input: &mut &str) -> ModalResult<i64> {
	(
		float,
		space1,
		take_while(1.., |c: char| c.is_ascii_alphabetic()),
	)
		.verify_map(|(value, _, unit): (f64, _, &str)| {
			unit_multiplier(unit).map(|mult| (value * mult).round() as i64)
		})
		.parse_next(input)
}

/// Bytes multiplier for a kopia size unit. kopia prints base-10 (`KB` = 1000);
/// binary spellings are accepted too in case a build uses them.
fn unit_multiplier(unit: &str) -> Option<f64> {
	Some(match unit {
		"B" => 1.0,
		"KB" => 1e3,
		"MB" => 1e6,
		"GB" => 1e9,
		"TB" => 1e12,
		"PB" => 1e15,
		"KiB" => 1024.0,
		"MiB" => 1024f64.powi(2),
		"GiB" => 1024f64.powi(3),
		"TiB" => 1024f64.powi(4),
		"PiB" => 1024f64.powi(5),
		_ => return None,
	})
}

/// Extract the byte figure immediately following `marker`, if present.
fn bytes_after(haystack: &str, marker: &str) -> Option<i64> {
	let idx = haystack.find(marker)? + marker.len();
	let mut rest = &haystack[idx..];
	bytes(&mut rest).ok()
}

/// Extract the integer that immediately precedes `noun` (e.g. `3 errors`),
/// unless it is qualified as ignored (handled separately).
fn error_count(haystack: &str, noun: &str) -> Option<i64> {
	let at = haystack.find(noun)?;
	let before = haystack[..at].trim_end();
	let ndigits = before
		.chars()
		.rev()
		.take_while(char::is_ascii_digit)
		.count();
	// Don't read an "ignored N errors" figure as the plain error figure.
	if before[..before.len() - ndigits]
		.trim_end()
		.ends_with("ignored")
	{
		return None;
	}
	trailing_integer(before)
}

/// Extract an ignored-errors figure, however kopia phrases it (`ignored N` or
/// `N ignored`).
fn ignored_count(haystack: &str) -> Option<i64> {
	if let Some(at) = haystack.find("ignored ") {
		let after = &haystack[at + "ignored ".len()..];
		if let Some(n) = leading_integer(after) {
			return Some(n);
		}
	}
	let at = haystack.find("ignored")?;
	trailing_integer(haystack[..at].trim_end())
}

/// The integer at the end of `s`, if it ends with digits.
fn trailing_integer(s: &str) -> Option<i64> {
	let digits: String = s
		.chars()
		.rev()
		.take_while(char::is_ascii_digit)
		.collect::<String>()
		.chars()
		.rev()
		.collect();
	digits.parse().ok()
}

/// The integer at the start of `s`, if it starts with digits.
fn leading_integer(s: &str) -> Option<i64> {
	let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
	digits.parse().ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_estimating_line() {
		// Real kopia 0.23.1 output.
		let p = parse_progress(
			" * 0 hashing, 3 hashed (140 MB), 0 cached (0 B), uploaded 137.5 MB, estimating...",
		)
		.expect("a progress line");
		assert_eq!(p.hashing_files, 0);
		assert_eq!(p.hashed_files, 3);
		assert_eq!(p.hashed_bytes, 140_000_000);
		assert_eq!(p.cached_files, 0);
		assert_eq!(p.cached_bytes, 0);
		assert_eq!(p.uploaded_bytes, 137_500_000);
		assert_eq!(p.estimated_bytes, None);
		assert_eq!(p.errors, None);
		assert_eq!(p.ignored_errors, None);
	}

	#[test]
	fn parses_mid_run_with_fractional_kb() {
		// Real kopia 0.23.1 output.
		let p = parse_progress(
			" | 6 hashing, 0 hashed (65.5 KB), 0 cached (0 B), uploaded 0 B, estimating...",
		)
		.expect("a progress line");
		assert_eq!(p.hashing_files, 6);
		assert_eq!(p.hashed_bytes, 65_500);
		assert_eq!(p.uploaded_bytes, 0);
	}

	#[test]
	fn parses_cached_line() {
		// Second snapshot of unchanged data: everything cached, nothing uploaded.
		let p = parse_progress(
			" * 0 hashing, 0 hashed (0 B), 3 cached (140 MB), uploaded 0 B, estimating...",
		)
		.expect("a progress line");
		assert_eq!(p.cached_files, 3);
		assert_eq!(p.cached_bytes, 140_000_000);
		assert_eq!(p.uploaded_bytes, 0);
	}

	#[test]
	fn parses_estimated_percent_tail() {
		// The tail once kopia's pre-scan estimate is ready.
		let p = parse_progress(
			" | 2 hashing, 15 hashed (1.2 GB), 3 cached (100 MB), uploaded 1.1 GB, estimated 5 GB (24.0%) 1m30s left",
		)
		.expect("a progress line");
		assert_eq!(p.hashed_bytes, 1_200_000_000);
		assert_eq!(p.cached_bytes, 100_000_000);
		assert_eq!(p.uploaded_bytes, 1_100_000_000);
		assert_eq!(p.estimated_bytes, Some(5_000_000_000));
	}

	#[test]
	fn extracts_errors() {
		let p = parse_progress(
			" - 0 hashing, 8 hashed (1 GB), 0 cached (0 B), uploaded 1 GB, 2 errors, estimating...",
		)
		.expect("a progress line");
		assert_eq!(p.errors, Some(2));
	}

	#[test]
	fn extracts_ignored_errors_without_double_counting() {
		let p = parse_progress(
			" - 0 hashing, 8 hashed (1 GB), 0 cached (0 B), uploaded 1 GB, ignored 4 errors, estimating...",
		)
		.expect("a progress line");
		// "ignored 4 errors" must not also be read as 4 plain errors.
		assert_eq!(p.ignored_errors, Some(4));
		assert_eq!(p.errors, None);
	}

	#[test]
	fn rejects_non_progress_lines() {
		assert!(parse_progress("Snapshotting felix@host:/tmp/data ...").is_none());
		assert!(parse_progress("Finished full maintenance.").is_none());
		assert!(parse_progress("GC found 0 unused contents (0 B)").is_none());
		assert!(parse_progress("").is_none());
	}
}
