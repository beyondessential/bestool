//! The session's segment writer.
//!
//! A session writes its records into one segment per UTC day, and into no file
//! any other session writes, so recording needs no coordination with anyone.
//!
//! spec: AUD, AUD-STO

use std::{collections::VecDeque, fs::OpenOptions, io::Write as _, path::PathBuf};

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use tracing::{debug, trace, warn};
use uuid::Uuid;

use super::{
	lock::Lock,
	paths,
	record::{
		ContextRecord, FORMAT_VERSION, GapRecord, QueryRecord, QuerySource, Record, RecordKind,
		frame, hash,
	},
	tailscale::{self, TailscalePeer},
};

/// How many unwritten records are held in memory before the oldest are dropped.
pub const BACKLOG_RECORDS: usize = 1000;

/// How many bytes of unwritten records are held before the oldest are dropped.
pub const BACKLOG_BYTES: usize = 16 * 1024 * 1024;

/// A record that has been made but not yet written.
///
/// Its sequence number and timestamp are fixed here, at the moment the record
/// was made, so a record that is never written leaves a hole in the numbering.
/// Its `prev` is not fixed here, because that names the record it will actually
/// end up following on disk.
#[derive(Debug, Clone)]
struct Pending {
	seq: u64,
	ts: Timestamp,
	kind: RecordKind,
	/// Rough serialised size, for bounding the backlog by bytes.
	weight: usize,
}

/// The records dropped from a full backlog, until a gap record accounts for them.
#[derive(Debug, Clone)]
struct Discarded {
	first_seq: u64,
	last_seq: u64,
	count: u64,
	from: Timestamp,
	to: Timestamp,
}

/// The segment currently open for writing.
#[derive(Debug)]
struct Segment {
	date: Date,
	path: PathBuf,
	lock: Lock,
}

/// Session state carried by context records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
	pub sys_user: String,
	pub db_user: String,
	pub writemode: bool,
	pub ots: Option<String>,
}

/// Writes one session's records into the audit log.
#[derive(Debug)]
pub struct Writer {
	dir: PathBuf,
	instance: Uuid,
	next_seq: u64,
	/// Hash of the last record written whole, which the next record chains onto.
	prev: String,
	segment: Option<Segment>,
	/// The day the records being made now belong to. A change starts a new
	/// segment, which opens with a context record.
	making_for: Option<Date>,
	/// The context last recorded, so a change can be noticed.
	context: Context,
	/// Peers sampled when the current segment opened.
	peers: Vec<TailscalePeer>,
	backlog: VecDeque<Pending>,
	backlog_bytes: usize,
	discarded: Option<Discarded>,
	/// A failing store is warned about once, then fails silently.
	warned: bool,
}

impl Writer {
	/// Open the log for writing, without touching the filesystem until the
	/// session first records something.
	pub fn new(dir: impl Into<PathBuf>) -> Self {
		Self {
			dir: dir.into(),
			instance: Uuid::new_v4(),
			next_seq: 0,
			prev: String::new(),
			segment: None,
			making_for: None,
			context: Context::default(),
			peers: Vec::new(),
			backlog: VecDeque::new(),
			backlog_bytes: 0,
			discarded: None,
			warned: false,
		}
	}

	pub fn instance(&self) -> Uuid {
		self.instance
	}

	/// Record a statement the session ran.
	///
	/// Never fails from the caller's point of view: a statement runs whether or
	/// not its record could be written, and a write failure never delays the
	/// prompt.
	pub fn query(&mut self, context: &Context, query: String, source: QuerySource) {
		self.make(
			Some(context),
			RecordKind::Query(QueryRecord { query, source }),
		);
	}

	/// Record a clean session exit.
	///
	/// A session that never recorded anything leaves no segment behind, so it
	/// has no end record to append either.
	pub fn end(&mut self) {
		if self.next_seq == 0 {
			return;
		}
		self.make(None, RecordKind::End);
	}

	/// Make a record, preceded by a context record where one is owed, and try to
	/// get everything held onto disk.
	fn make(&mut self, context: Option<&Context>, kind: RecordKind) {
		let now = Timestamp::now();
		let date = date_of(now);

		// Crossing into a new UTC day starts a new segment, and the first
		// record of every segment is a context record. Peers stand as who was
		// reachable when the segment opened, so they are sampled here rather
		// than per record: the prompt never waits on a subprocess more than
		// once a day.
		let rolled = self.making_for != Some(date);
		if rolled {
			debug!(?date, "audit segment day rolled");
			self.making_for = Some(date);
			self.peers = tailscale::get_active_peers().unwrap_or_default();
		}

		let changed = context.is_some_and(|context| *context != self.context);
		if let Some(context) = context {
			self.context = context.clone();
		}
		if rolled || changed {
			self.enqueue(self.context_record());
		}

		self.enqueue(kind);
		self.pump();
	}

	fn context_record(&self) -> RecordKind {
		RecordKind::Context(ContextRecord {
			sys_user: self.context.sys_user.clone(),
			db_user: self.context.db_user.clone(),
			writemode: self.context.writemode,
			ots: self.context.ots.clone(),
			tailscale: self.peers.clone(),
			instance: self.instance,
		})
	}

	/// Assign a record its sequence number and timestamp and hold it for writing.
	fn enqueue(&mut self, kind: RecordKind) {
		let seq = self.next_seq;
		self.next_seq += 1;
		let weight = weigh(&kind);

		self.backlog.push_back(Pending {
			seq,
			ts: Timestamp::now(),
			kind,
			weight,
		});
		self.backlog_bytes += weight;
		self.trim();
	}

	/// Try to write everything held, warning at most once per session.
	fn pump(&mut self) {
		let Err(err) = self.flush() else { return };

		if !self.warned {
			self.warned = true;
			eprintln!("warning: the audit log cannot be written: {err}");
			eprintln!("warning: statements still run, but are no longer being recorded");
		}
		trace!(?err, "audit write failed");
	}

	/// Drop the oldest held records once either bound is reached, tallying them
	/// so a gap record can account for them when recording resumes.
	fn trim(&mut self) {
		while self.backlog.len() > BACKLOG_RECORDS || self.backlog_bytes > BACKLOG_BYTES {
			// One record over the byte bound on its own has nothing older to
			// drop, and dropping it would make a single huge statement the one
			// thing that can never be recorded.
			if self.backlog.len() <= 1 {
				break;
			}

			let Some(dropped) = self.backlog.pop_front() else {
				break;
			};
			self.backlog_bytes = self.backlog_bytes.saturating_sub(dropped.weight);

			match &mut self.discarded {
				Some(tally) => {
					tally.last_seq = tally.last_seq.max(dropped.seq);
					tally.count += 1;
					tally.from = tally.from.min(dropped.ts);
					tally.to = tally.to.max(dropped.ts);
				}
				None => {
					self.discarded = Some(Discarded {
						first_seq: dropped.seq,
						last_seq: dropped.seq,
						count: 1,
						from: dropped.ts,
						to: dropped.ts,
					})
				}
			}
		}
	}

	/// Write out everything held, oldest first, behind a gap record if any were
	/// lost since the last successful write.
	fn flush(&mut self) -> Result<()> {
		if let Some(tally) = self.discarded.clone() {
			// The gap takes the first of the sequence numbers it covers, so the
			// numbering in a segment stays in order, and the time of the last
			// record it covers, so it still sorts before the held records that
			// survived and are about to be written behind it. Write order,
			// sequence order and time order therefore agree, which is what lets
			// a time-ordered export verify.
			self.write(&Pending {
				seq: tally.first_seq,
				ts: tally.to,
				kind: RecordKind::Gap(GapRecord {
					lost: tally.count,
					through: tally.last_seq,
					from: tally.from,
					to: tally.to,
				}),
				weight: 0,
			})?;
			self.discarded = None;
		}

		while let Some(pending) = self.backlog.front().cloned() {
			self.write(&pending)?;
			self.backlog.pop_front();
			self.backlog_bytes = self.backlog_bytes.saturating_sub(pending.weight);
		}

		Ok(())
	}

	/// Write one record into the segment for the day it was made on.
	fn write(&mut self, pending: &Pending) -> Result<()> {
		self.open_segment(date_of(pending.ts))?;

		let record = Record {
			v: FORMAT_VERSION,
			seq: pending.seq,
			ts: pending.ts,
			prev: self.prev.clone(),
			kind: pending.kind.clone(),
		};
		let json = record.to_json().into_diagnostic()?;

		let segment = self.segment.as_mut().expect("just opened");
		segment
			.lock
			.file_mut()
			.write_all(&frame(&json))
			.into_diagnostic()
			.wrap_err_with(|| format!("appending to {}", segment.path.display()))?;

		self.prev = hash(&json);
		Ok(())
	}

	/// Make the segment for `date` the open one.
	///
	/// This rolls over at midnight, and also reopens an earlier day when a
	/// held-back record belongs to one: a record goes to the segment for its
	/// own day, so no segment ever spans a date boundary.
	fn open_segment(&mut self, date: Date) -> Result<()> {
		if self.segment.as_ref().is_some_and(|open| open.date == date) {
			return Ok(());
		}

		// Dropping the previous segment releases its lock, which is what tells
		// compaction that day is closed.
		self.segment = None;

		std::fs::create_dir_all(&self.dir)
			.into_diagnostic()
			.wrap_err_with(|| format!("creating audit directory {}", self.dir.display()))?;

		let path = self.dir.join(paths::segment_name(date, self.instance));
		let file = OpenOptions::new()
			.create(true)
			.append(true)
			.read(true)
			.open(&path)
			.into_diagnostic()
			.wrap_err_with(|| format!("opening audit segment {}", path.display()))?;

		let lock = Lock::try_hold(file)?
			.ok_or_else(|| miette!("audit segment {} is held by another writer", path.display()))?;

		self.segment = Some(Segment { date, path, lock });
		Ok(())
	}
}

impl Drop for Writer {
	fn drop(&mut self) {
		// A clean exit appends an end record; a crash leaves none, which is how
		// a reader tells the two apart.
		self.end();
		if !self.backlog.is_empty() {
			warn!(
				held = self.backlog.len(),
				"audit records could not be written before exit"
			);
		}
	}
}

/// The UTC day a timestamp falls in, which is what a segment covers.
pub fn date_of(ts: Timestamp) -> Date {
	ts.to_zoned(TimeZone::UTC).date()
}

/// Rough serialised size of a record, for bounding the backlog by bytes.
fn weigh(kind: &RecordKind) -> usize {
	const OVERHEAD: usize = 128;
	OVERHEAD
		+ match kind {
			RecordKind::Query(query) => query.query.len(),
			RecordKind::Context(context) => {
				context.sys_user.len()
					+ context.db_user.len()
					+ context.ots.as_ref().map_or(0, String::len)
					+ context.tailscale.len() * 64
			}
			RecordKind::Gap(_) | RecordKind::End => 0,
		}
}

#[cfg(test)]
pub(crate) fn read_records(path: &std::path::Path) -> Vec<Record> {
	use super::read::FramedReader;
	use std::io::BufReader;

	FramedReader::new(BufReader::new(std::fs::File::open(path).unwrap()))
		.filter_map(|item| item.into_record())
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn context() -> Context {
		Context {
			sys_user: "felix".into(),
			db_user: "tamanu".into(),
			writemode: false,
			ots: None,
		}
	}

	fn files(dir: &std::path::Path) -> Vec<PathBuf> {
		paths::list(dir)
			.unwrap()
			.into_iter()
			.map(|(path, _)| path)
			.collect()
	}

	fn queries(records: &[Record]) -> Vec<String> {
		records
			.iter()
			.filter_map(|r| match &r.kind {
				RecordKind::Query(q) => Some(q.query.clone()),
				_ => None,
			})
			.collect()
	}

	#[test]
	fn a_session_writes_one_segment_opening_with_a_context_record() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "select 1;".into(), QuerySource::Typed);
		writer.query(&context(), "select 2;".into(), QuerySource::Typed);
		drop(writer);

		let found = files(dir.path());
		assert_eq!(found.len(), 1);

		let records = read_records(&found[0]);
		assert!(matches!(records[0].kind, RecordKind::Context(_)));
		assert_eq!(queries(&records), vec!["select 1;", "select 2;"]);
		assert!(matches!(records.last().unwrap().kind, RecordKind::End));
	}

	#[test]
	fn the_chain_links_every_record_and_only_the_first_is_empty() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		for i in 0..5 {
			writer.query(&context(), format!("select {i};"), QuerySource::Typed);
		}
		drop(writer);

		let records = read_records(&files(dir.path())[0]);
		assert!(records[0].prev.is_empty());
		for pair in records.windows(2) {
			assert_eq!(
				pair[1].prev,
				hash(&pair[0].to_json().unwrap()),
				"record {} chains onto {}",
				pair[1].seq,
				pair[0].seq
			);
		}
	}

	#[test]
	fn sequence_numbers_are_contiguous_from_zero() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "select 1;".into(), QuerySource::Typed);
		writer.query(&context(), "select 2;".into(), QuerySource::Typed);
		drop(writer);

		let records = read_records(&files(dir.path())[0]);
		let seqs: Vec<_> = records.iter().map(|r| r.seq).collect();
		assert_eq!(seqs, vec![0, 1, 2, 3]);
	}

	#[test]
	fn a_context_change_is_recorded_before_the_next_query() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "select 1;".into(), QuerySource::Typed);

		let supervised = Context {
			writemode: true,
			ots: Some("Dr Who".into()),
			..context()
		};
		writer.query(&supervised, "update patients;".into(), QuerySource::Typed);
		drop(writer);

		let records = read_records(&files(dir.path())[0]);
		let contexts: Vec<_> = records
			.iter()
			.filter_map(|r| match &r.kind {
				RecordKind::Context(c) => Some(c.clone()),
				_ => None,
			})
			.collect();
		assert_eq!(contexts.len(), 2);
		assert!(!contexts[0].writemode);
		assert!(contexts[1].writemode);
		assert_eq!(contexts[1].ots.as_deref(), Some("Dr Who"));

		// The new context lands before the query it applies to.
		let context_at = records
			.iter()
			.position(|r| matches!(&r.kind, RecordKind::Context(c) if c.writemode))
			.unwrap();
		let query_at = records
			.iter()
			.position(|r| matches!(&r.kind, RecordKind::Query(q) if q.query == "update patients;"))
			.unwrap();
		assert!(context_at < query_at);
	}

	#[test]
	fn an_unchanged_context_is_not_recorded_again() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		for i in 0..5 {
			writer.query(&context(), format!("select {i};"), QuerySource::Typed);
		}
		drop(writer);

		let records = read_records(&files(dir.path())[0]);
		assert_eq!(
			records
				.iter()
				.filter(|r| matches!(r.kind, RecordKind::Context(_)))
				.count(),
			1
		);
	}

	#[test]
	fn a_statements_source_is_recorded() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "select 1;".into(), QuerySource::Typed);
		writer.query(
			&context(),
			"select 2;".into(),
			QuerySource::Snippet {
				name: "counts".into(),
			},
		);
		writer.query(
			&context(),
			"select 3;".into(),
			QuerySource::Include {
				path: "/tmp/fixups.sql".into(),
			},
		);
		drop(writer);

		let records = read_records(&files(dir.path())[0]);
		let sources: Vec<_> = records
			.iter()
			.filter_map(|r| match &r.kind {
				RecordKind::Query(q) => Some(q.source.clone()),
				_ => None,
			})
			.collect();
		assert_eq!(
			sources,
			vec![
				QuerySource::Typed,
				QuerySource::Snippet {
					name: "counts".into()
				},
				QuerySource::Include {
					path: "/tmp/fixups.sql".into()
				},
			]
		);
	}

	#[test]
	fn the_segment_is_named_for_its_day_and_session() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		let instance = writer.instance();
		writer.query(&context(), "select 1;".into(), QuerySource::Typed);
		drop(writer);

		let name = files(dir.path())[0]
			.file_name()
			.unwrap()
			.to_str()
			.unwrap()
			.to_owned();
		assert_eq!(
			paths::classify(&name),
			Some(paths::AuditFile::Segment {
				date: date_of(Timestamp::now()),
				instance
			})
		);
	}

	#[test]
	fn concurrent_sessions_write_separate_segments_without_coordinating() {
		let dir = tempfile::tempdir().unwrap();
		let mut one = Writer::new(dir.path());
		let mut two = Writer::new(dir.path());

		one.query(&context(), "from one;".into(), QuerySource::Typed);
		two.query(&context(), "from two;".into(), QuerySource::Typed);
		one.query(&context(), "one again;".into(), QuerySource::Typed);
		two.query(&context(), "two again;".into(), QuerySource::Typed);
		drop(one);
		drop(two);

		let found = files(dir.path());
		assert_eq!(found.len(), 2);

		let mut all: Vec<String> = found
			.iter()
			.flat_map(|p| queries(&read_records(p)))
			.collect();
		all.sort();
		assert_eq!(
			all,
			vec!["from one;", "from two;", "one again;", "two again;"]
		);
	}

	#[test]
	fn a_live_segment_is_locked_and_a_closed_one_is_not() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "select 1;".into(), QuerySource::Typed);

		let path = files(dir.path())[0].clone();
		assert!(!super::super::lock::is_free(&path));
		drop(writer);
		assert!(super::super::lock::is_free(&path));
	}

	#[test]
	fn a_session_that_records_nothing_leaves_no_segment() {
		let dir = tempfile::tempdir().unwrap();
		drop(Writer::new(dir.path()));
		assert!(files(dir.path()).is_empty());
	}

	#[test]
	fn recording_survives_an_unwritable_store_and_resumes_with_a_gap() {
		let dir = tempfile::tempdir().unwrap();
		// A plain file where the store directory should be: every write fails,
		// as it would on a filesystem that has gone away mid-incident.
		let store = dir.path().join("store");
		std::fs::write(&store, b"in the way").unwrap();

		let mut writer = Writer::new(&store);
		for i in 0..(BACKLOG_RECORDS + 50) {
			writer.query(&context(), format!("lost {i};"), QuerySource::Typed);
		}
		assert!(
			writer.discarded.is_some(),
			"the bounded backlog dropped its oldest records"
		);

		std::fs::remove_file(&store).unwrap();
		writer.query(&context(), "after;".into(), QuerySource::Typed);
		drop(writer);

		let records = read_records(&files(&store)[0]);
		let (gap_seq, gap) = records
			.iter()
			.find_map(|r| match &r.kind {
				RecordKind::Gap(g) => Some((r.seq, g.clone())),
				_ => None,
			})
			.expect("a gap record accounts for the lost records");

		// The gap takes the first sequence number it covers and names the last,
		// so the numbering stays in order and the span is stated.
		assert_eq!(gap.through, gap_seq + gap.lost - 1);
		assert!(gap.from <= gap.to);

		// It is the first record in the file, and the surviving backlog follows
		// it in the order those records were made.
		assert_eq!(records[0].seq, gap_seq);
		let survivors = queries(&records);
		assert_eq!(survivors.last().unwrap(), "after;");
		let mut sorted = survivors.clone();
		sorted.sort_by_key(|q| {
			q.trim_start_matches("lost ")
				.trim_end_matches(';')
				.parse::<u64>()
				.unwrap_or(u64::MAX)
		});
		assert_eq!(
			survivors, sorted,
			"held records flush in their original order"
		);
	}

	#[test]
	fn the_chain_holds_across_a_gap() {
		let dir = tempfile::tempdir().unwrap();
		let store = dir.path().join("store");
		std::fs::write(&store, b"in the way").unwrap();

		let mut writer = Writer::new(&store);
		for i in 0..(BACKLOG_RECORDS + 5) {
			writer.query(&context(), format!("lost {i};"), QuerySource::Typed);
		}
		std::fs::remove_file(&store).unwrap();
		writer.query(&context(), "after;".into(), QuerySource::Typed);
		drop(writer);

		let records = read_records(&files(&store)[0]);
		for pair in records.windows(2) {
			assert_eq!(pair[1].prev, hash(&pair[0].to_json().unwrap()));
		}
	}

	#[test]
	fn a_never_writable_store_never_panics() {
		let dir = tempfile::tempdir().unwrap();
		let store = dir.path().join("store");
		std::fs::write(&store, b"in the way").unwrap();

		let mut writer = Writer::new(&store);
		for i in 0..10 {
			writer.query(&context(), format!("select {i};"), QuerySource::Typed);
		}
		drop(writer);
	}
}
