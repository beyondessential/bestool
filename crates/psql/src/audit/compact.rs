//! Compaction and retention.
//!
//! Compaction folds each day's segments into a single day file so the directory
//! stays small, and retention deletes days older than the retention period.
//! Neither ever touches a live segment, and neither ever runs where a session
//! could be waiting on it.
//!
//! spec: AUD-RET

use std::{
	collections::{BTreeSet, HashSet},
	fs::{File, OpenOptions},
	io::{BufReader, Seek as _, SeekFrom, Write as _},
	path::{Path, PathBuf},
};

use jiff::{Span, Timestamp, civil::Date, tz::TimeZone};
use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use thread_priority::{ThreadBuilderExt as _, ThreadPriority};
use tracing::{debug, info, warn};

use super::{
	lock::Lock,
	paths::{self, AuditFile},
	read::{FramedItem, FramedReader, open},
	record::{Record, frame},
};

/// How long segments are left uncompacted after the day they cover, so the last
/// fortnight of the log can always be read with ordinary text tools.
pub const PLAIN_TEXT_WINDOW_DAYS: i32 = 15;

/// How long records are kept: the organisation's retention period for
/// security-sensitive audit logs.
pub const RETENTION_MONTHS: i32 = 12;

/// How many days one run folds, so a session's startup never turns into a long
/// job on a directory that has gone a long time without compaction.
pub const DAYS_PER_RUN: usize = 1;

/// Compression level. Audit records are highly repetitive JSON, so the default
/// already compresses them far down; a higher level would cost time for little.
const LEVEL: i32 = 3;

/// What one run folded and deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactionReport {
	/// Days folded into a day file, with how many segments each consumed.
	pub folded: Vec<(Date, usize)>,
	/// Day files deleted because they fell outside the retention period.
	pub expired: Vec<Date>,
	/// Whether another process held the directory, so this run did nothing.
	pub skipped: bool,
}

impl CompactionReport {
	pub fn did_nothing(&self) -> bool {
		self.folded.is_empty() && self.expired.is_empty()
	}
}

/// Run compaction and retention once.
///
/// Takes the directory lock for its duration and skips rather than waits when
/// another process holds it.
pub fn run(dir: &Path) -> Result<CompactionReport> {
	let Some(lock) = Lock::try_directory(dir)? else {
		debug!("audit directory is held by another process, skipping compaction");
		return Ok(CompactionReport {
			skipped: true,
			..Default::default()
		});
	};

	// A store still in the old single-file format is brought across first, so
	// what it held takes part in compaction like anything else.
	super::legacy::import(dir, &lock)?;

	let today = Timestamp::now().to_zoned(TimeZone::UTC).date();
	let mut report = CompactionReport::default();

	for date in eligible(dir, today)?.into_iter().take(DAYS_PER_RUN) {
		match fold(dir, date) {
			Ok(0) => debug!(%date, "nothing folded for this day"),
			Ok(consumed) => report.folded.push((date, consumed)),
			Err(err) => warn!(?err, %date, "could not fold audit segments"),
		}
	}

	report.expired = expire(dir, today)?;
	Ok(report)
}

/// Whether there is anything for compaction to do, without taking any locks.
///
/// A session runs compaction at startup, so this has to be cheap enough to run
/// every time and do nothing when there is nothing eligible.
pub fn is_worthwhile(dir: &Path) -> bool {
	let today = Timestamp::now().to_zoned(TimeZone::UTC).date();
	let Ok(files) = paths::list(dir) else {
		return false;
	};

	let expiring = files.iter().any(|(_, kind)| {
		matches!(kind, AuditFile::DayFile { .. }) && kind.date() < retention_cutoff(today)
	});
	let foldable = files.iter().any(|(_, kind)| {
		matches!(kind, AuditFile::Segment { .. }) && kind.date() < window_cutoff(today)
	});
	let legacy = paths::list_legacy(dir).is_ok_and(|found| !found.is_empty());

	expiring || foldable || legacy
}

/// Days whose segments are eligible for compaction, oldest first.
///
/// A day's segments become eligible once that day ended longer ago than the
/// plain-text window. Because the window is a span of days rather than a
/// calendar boundary, the stretch of log held in plain text is the same on
/// every date.
fn eligible(dir: &Path, today: Date) -> Result<Vec<Date>> {
	let cutoff = window_cutoff(today);
	let mut days = BTreeSet::new();

	for (_, kind) in paths::list(dir)? {
		let AuditFile::Segment { date, .. } = kind else {
			continue;
		};
		if date < cutoff {
			days.insert(date);
		}
	}

	Ok(days.into_iter().collect())
}

/// Fold one day's segments, and any day file already there, into a day file.
///
/// Writes under a temporary name, synchronises, renames into place, and only
/// then deletes the segments it consumed, so an interruption at any point leaves
/// the log with duplicated records rather than missing ones.
fn fold(dir: &Path, date: Date) -> Result<usize> {
	let mut sources: Vec<(PathBuf, AuditFile)> = paths::list(dir)?
		.into_iter()
		.filter(|(_, kind)| kind.date() == date)
		.collect();
	// An interrupted run can leave a day file beside segments it had not yet
	// consumed; taking it as a source too makes the rebuild complete.
	sources.sort_by_key(|(_, kind)| kind.instance().is_none());

	let segments: Vec<PathBuf> = sources
		.iter()
		.filter(|(_, kind)| kind.instance().is_some())
		.map(|(path, _)| path.clone())
		.collect();
	if segments.is_empty() {
		return Ok(0);
	}

	// Each segment's lock is taken and held for the whole fold, not merely
	// tested: a session flushing held records can reopen an earlier day, so a
	// segment that was free a moment ago can be live again by the time it would
	// be deleted. A day whose segments cannot all be taken is left alone
	// entirely rather than folded by halves.
	let mut held = Vec::with_capacity(segments.len());
	for path in &segments {
		let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
			debug!(
				?path,
				"segment cannot be opened for compaction, leaving the day"
			);
			return Ok(0);
		};
		match Lock::try_hold(file)? {
			Some(lock) => held.push((path.clone(), lock)),
			None => {
				debug!(%date, ?path, "a segment for this day is still live, leaving it");
				return Ok(0);
			}
		}
	}

	let mut records: Vec<(Record, String)> = Vec::new();
	for (path, kind) in &sources {
		// A segment is read back through the very handle its lock was taken on.
		// Opening a second one would be refused on Windows, where the exclusive
		// lock just taken is enforced rather than advisory.
		let items: Box<dyn Iterator<Item = FramedItem>> =
			match held.iter_mut().find(|(held_path, _)| held_path == path) {
				Some((_, lock)) => {
					lock.file_mut()
						.seek(SeekFrom::Start(0))
						.into_diagnostic()
						.wrap_err_with(|| format!("rewinding {}", path.display()))?;
					Box::new(FramedReader::new(BufReader::new(lock.file_mut())))
				}
				None => open(path, *kind)?,
			};

		for item in items {
			match item {
				FramedItem::Record { record, json, .. } => records.push((record, json)),
				FramedItem::Skipped { at, bytes } => {
					// Bytes that are not a record take no part in the chain, so
					// they are dropped rather than carried into the day file.
					warn!(
						?path,
						at, bytes, "dropping unparsable bytes during compaction"
					);
				}
				// What was read is not known to be all the file holds, and
				// folding would delete the original, so the day is left alone.
				FramedItem::Failed { at, error } => {
					return Err(miette!(
						"{} stopped being readable at byte {at}: {error}",
						path.display()
					));
				}
			}
		}
	}

	// Records inside a day file are the same records, with the same fields,
	// framing and hash chain as they had in their segments: only the container
	// changes. Copies left by an earlier interrupted run are byte-identical, so
	// folding them together is exact.
	records.sort_by_key(|(record, _)| (record.ts, record.seq));
	let mut seen = HashSet::new();
	records.retain(|(_, json)| seen.insert(super::record::hash(json)));

	let temp = dir.join(format!(
		"{}{}",
		paths::day_file_name(date),
		paths::TEMP_SUFFIX
	));
	let final_path = dir.join(paths::day_file_name(date));

	{
		let file = File::create(&temp)
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", temp.display()))?;
		let mut encoder = zstd::Encoder::new(file, LEVEL).into_diagnostic()?;
		for (_, json) in &records {
			encoder.write_all(&frame(json)).into_diagnostic()?;
		}
		let file = encoder.finish().into_diagnostic()?;
		file.sync_all()
			.into_diagnostic()
			.wrap_err_with(|| format!("synchronising {}", temp.display()))?;
	}

	std::fs::rename(&temp, &final_path)
		.into_diagnostic()
		.wrap_err_with(|| format!("renaming {} into place", temp.display()))?;

	// Each segment's lock is released only as that segment goes, and not before
	// the day file naming it is in place: deleting a file that is still open is
	// refused on some platforms.
	let consumed = held.len();
	for (path, lock) in held {
		drop(lock);
		if let Err(err) = std::fs::remove_file(&path) {
			warn!(?err, ?path, "could not delete a folded audit segment");
		}
	}

	info!(%date, consumed, records = records.len(), "folded audit segments into a day file");
	Ok(consumed)
}

/// Delete day files whose day ended longer ago than the retention period.
fn expire(dir: &Path, today: Date) -> Result<Vec<Date>> {
	let cutoff = retention_cutoff(today);
	let mut deleted = Vec::new();

	for (path, kind) in paths::list(dir)? {
		let AuditFile::DayFile { date } = kind else {
			continue;
		};
		if date >= cutoff {
			continue;
		}
		match std::fs::remove_file(&path) {
			Ok(()) => {
				info!(%date, "deleted an audit day file past its retention period");
				deleted.push(date);
			}
			Err(err) => warn!(?err, ?path, "could not delete an expired audit day file"),
		}
	}

	Ok(deleted)
}

/// Days before this are eligible for folding.
///
/// Both cutoffs fall back to the earliest representable date rather than to
/// today, so arithmetic that cannot be done leaves the log alone instead of
/// folding or deleting all of it.
fn window_cutoff(today: Date) -> Date {
	today
		.checked_sub(Span::new().days(PLAIN_TEXT_WINDOW_DAYS))
		.unwrap_or(Date::MIN)
}

/// Day files covering a day before this have outlived the retention period.
fn retention_cutoff(today: Date) -> Date {
	today
		.checked_sub(Span::new().months(RETENTION_MONTHS))
		.unwrap_or(Date::MIN)
}

/// Run compaction in the background, at low priority so it does not compete
/// with the prompt, and only when there is something eligible.
pub fn spawn(dir: PathBuf) {
	if !is_worthwhile(&dir) {
		debug!("nothing eligible for audit compaction");
		return;
	}

	let spawned = std::thread::Builder::new()
		.name("audit-compaction".into())
		.spawn_with_priority(ThreadPriority::Min, move |priority| {
			if let Err(err) = priority {
				debug!(?err, "could not lower audit compaction thread priority");
			}
			match run(&dir) {
				Ok(report) if report.did_nothing() => debug!("audit compaction folded nothing"),
				Ok(report) => debug!(?report, "audit compaction done"),
				Err(err) => warn!(?err, "audit compaction failed"),
			}
		});

	if let Err(err) = spawned {
		debug!(?err, "could not spawn audit compaction");
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::audit::{
		read::Reader,
		record::{QuerySource, RecordKind},
		verify::verify,
		writer::{Context, Writer},
	};

	fn context() -> Context {
		Context {
			sys_user: "felix".into(),
			db_user: "tamanu".into(),
			writemode: false,
			ots: None,
		}
	}

	/// Write a session's worth of records, then rename its segment to a day in
	/// the past so it becomes eligible.
	fn session_on(dir: &Path, date: Date, queries: &[&str]) {
		let mut writer = Writer::new(dir);
		let instance = writer.instance();
		for query in queries {
			writer.query(&context(), (*query).into(), QuerySource::Typed);
		}
		drop(writer);

		let today = Timestamp::now().to_zoned(TimeZone::UTC).date();
		let from = dir.join(paths::segment_name(today, instance));
		let to = dir.join(paths::segment_name(date, instance));
		std::fs::rename(from, to).unwrap();
	}

	fn days_ago(days: i32) -> Date {
		Timestamp::now()
			.to_zoned(TimeZone::UTC)
			.date()
			.checked_sub(Span::new().days(days))
			.unwrap()
	}

	fn queries(dir: &Path) -> Vec<String> {
		Reader::open(dir)
			.unwrap()
			.entries()
			.map(|e| e.query)
			.collect()
	}

	#[test]
	fn segments_inside_the_plain_text_window_are_left_alone() {
		let dir = tempfile::tempdir().unwrap();
		session_on(
			dir.path(),
			days_ago(PLAIN_TEXT_WINDOW_DAYS - 1),
			&["recent;"],
		);

		let report = run(dir.path()).unwrap();
		assert!(report.folded.is_empty());
		assert!(
			paths::list(dir.path())
				.unwrap()
				.iter()
				.all(|(_, kind)| matches!(kind, AuditFile::Segment { .. })),
			"the last fortnight stays plain text"
		);
	}

	#[test]
	fn a_day_past_the_window_is_folded_into_a_day_file() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), date, &["select 1;", "select 2;"]);

		let report = run(dir.path()).unwrap();
		assert_eq!(report.folded, vec![(date, 1)]);

		let found = paths::list(dir.path()).unwrap();
		assert_eq!(found.len(), 1);
		assert_eq!(found[0].1, AuditFile::DayFile { date });
		assert_eq!(queries(dir.path()), vec!["select 1;", "select 2;"]);
	}

	#[test]
	fn folding_preserves_the_records_and_their_chain() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), date, &["select 1;", "select 2;", "select 3;"]);

		let before: Vec<_> = Reader::open(dir.path())
			.unwrap()
			.map(|s| (s.json, s.hash))
			.collect();
		run(dir.path()).unwrap();
		let after: Vec<_> = Reader::open(dir.path())
			.unwrap()
			.map(|s| (s.json, s.hash))
			.collect();

		assert_eq!(
			before, after,
			"compaction changes the container, not the content"
		);
		assert!(verify(dir.path()).unwrap().holds());
	}

	#[test]
	fn several_sessions_on_one_day_fold_into_one_file_in_time_order() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 2);
		session_on(dir.path(), date, &["one a;", "one b;"]);
		session_on(dir.path(), date, &["two a;", "two b;"]);

		let expected = queries(dir.path());
		let report = run(dir.path()).unwrap();
		assert_eq!(report.folded, vec![(date, 2)]);
		assert_eq!(paths::list(dir.path()).unwrap().len(), 1);
		assert_eq!(queries(dir.path()), expected);
		assert!(verify(dir.path()).unwrap().holds());
	}

	#[test]
	fn a_live_segment_keeps_its_whole_day_out_of_compaction() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), date, &["closed;"]);

		// A session that has been idle since before the window still holds its
		// segment open.
		let mut live = Writer::new(dir.path());
		live.query(&context(), "live;".into(), QuerySource::Typed);
		let instance = live.instance();
		let today = Timestamp::now().to_zoned(TimeZone::UTC).date();
		// The live writer keeps the file handle, so a rename leaves it locked.
		std::fs::rename(
			dir.path().join(paths::segment_name(today, instance)),
			dir.path().join(paths::segment_name(date, instance)),
		)
		.unwrap();

		let report = run(dir.path()).unwrap();
		assert!(report.folded.is_empty(), "the day is left alone entirely");
		drop(live);
	}

	#[test]
	fn only_one_day_is_folded_per_run() {
		let dir = tempfile::tempdir().unwrap();
		let older = days_ago(PLAIN_TEXT_WINDOW_DAYS + 3);
		let newer = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), older, &["older;"]);
		session_on(dir.path(), newer, &["newer;"]);

		let first = run(dir.path()).unwrap();
		assert_eq!(first.folded, vec![(older, 1)], "the oldest day goes first");

		let second = run(dir.path()).unwrap();
		assert_eq!(second.folded, vec![(newer, 1)]);
	}

	#[test]
	fn an_interrupted_fold_leaves_duplicates_that_read_as_one_record() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), date, &["select 1;", "select 2;"]);

		let expected = queries(dir.path());
		let segment = paths::list(dir.path()).unwrap()[0].0.clone();
		let kept = std::fs::read(&segment).unwrap();

		run(dir.path()).unwrap();
		// Put the segment back, as an interruption between the rename and the
		// delete would have.
		std::fs::write(&segment, kept).unwrap();

		assert_eq!(queries(dir.path()), expected, "duplicates fold together");
		assert!(verify(dir.path()).unwrap().holds());

		// And a later run tidies up without losing anything.
		run(dir.path()).unwrap();
		assert_eq!(queries(dir.path()), expected);
		assert_eq!(paths::list(dir.path()).unwrap().len(), 1);
	}

	#[test]
	fn a_day_that_cannot_be_read_in_full_is_not_folded() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), date, &["select 1;", "select 2;"]);

		// A day file that stops decompressing partway is a read that failed,
		// not a file that ended: folding it would write back only what was read
		// and then delete the segments the rest came from.
		let truncated = dir.path().join(paths::day_file_name(date));
		let mut bytes = zstd::encode_all(&b"\x1e{\"v\":1}\n"[..], 3).unwrap();
		bytes.truncate(bytes.len() / 2);
		std::fs::write(&truncated, bytes).unwrap();

		let report = run(dir.path()).unwrap();
		assert!(report.folded.is_empty(), "the day is left alone");
		assert!(
			paths::list(dir.path())
				.unwrap()
				.iter()
				.any(|(_, kind)| kind.instance().is_some()),
			"the segments the records came from are still there"
		);
	}

	#[test]
	fn a_live_segment_reopened_after_the_check_is_not_deleted() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), date, &["closed;"]);

		// Hold the segment as a writer flushing held records to an earlier day
		// would, after eligibility has already been decided.
		let path = paths::list(dir.path()).unwrap()[0].0.clone();
		let file = std::fs::OpenOptions::new()
			.read(true)
			.write(true)
			.open(&path)
			.unwrap();
		let held = Lock::try_hold(file).unwrap().unwrap();

		let report = run(dir.path()).unwrap();
		assert!(report.folded.is_empty());
		assert!(path.exists(), "a held segment is never deleted");
		drop(held);
	}

	#[test]
	fn a_day_file_past_the_retention_period_is_deleted() {
		let dir = tempfile::tempdir().unwrap();
		let old = days_ago(RETENTION_MONTHS * 31 + 10);
		session_on(dir.path(), old, &["ancient;"]);

		// One run folds the day and then finds the day file already past the
		// retention period.
		let report = run(dir.path()).unwrap();
		assert_eq!(report.folded, vec![(old, 1)]);
		assert_eq!(report.expired, vec![old]);
		assert!(paths::list(dir.path()).unwrap().is_empty());
	}

	#[test]
	fn a_day_file_inside_the_retention_period_is_kept() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), date, &["keep me;"]);

		run(dir.path()).unwrap();
		let report = run(dir.path()).unwrap();
		assert!(report.expired.is_empty());
		assert_eq!(queries(dir.path()), vec!["keep me;"]);
	}

	#[test]
	fn compaction_skips_rather_than_waits_when_the_directory_is_held() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), date, &["select 1;"]);

		let held = Lock::try_directory(dir.path()).unwrap().unwrap();
		let report = run(dir.path()).unwrap();
		assert!(report.skipped);
		assert!(report.folded.is_empty());
		drop(held);

		assert!(!run(dir.path()).unwrap().skipped);
	}

	#[test]
	fn there_is_nothing_worthwhile_in_a_fresh_directory() {
		let dir = tempfile::tempdir().unwrap();
		assert!(!is_worthwhile(dir.path()));

		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "select 1;".into(), QuerySource::Typed);
		drop(writer);
		assert!(
			!is_worthwhile(dir.path()),
			"today's segment is not eligible"
		);

		session_on(dir.path(), days_ago(PLAIN_TEXT_WINDOW_DAYS + 1), &["old;"]);
		assert!(is_worthwhile(dir.path()));
	}

	#[test]
	fn an_end_record_survives_folding() {
		let dir = tempfile::tempdir().unwrap();
		let date = days_ago(PLAIN_TEXT_WINDOW_DAYS + 1);
		session_on(dir.path(), date, &["select 1;"]);
		run(dir.path()).unwrap();

		assert!(
			Reader::open(dir.path())
				.unwrap()
				.any(|s| matches!(s.record.kind, RecordKind::End)),
			"a clean exit is still visible after compaction"
		);
	}
}
