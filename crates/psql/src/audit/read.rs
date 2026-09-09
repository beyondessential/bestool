//! Reading the audit log.
//!
//! The log is read as one time-ordered stream of records across every segment
//! and day file. Two surfaces sit over that one merge: the records as they are
//! stored, which is what export writes out, and flat entries, where each query
//! record carries the context in force at it.
//!
//! spec: AUD-STO, AUD-API

use std::{
	cmp::Reverse,
	collections::{BinaryHeap, HashMap, HashSet, VecDeque},
	fs::File,
	io::{BufRead, BufReader, Read, Seek, SeekFrom},
	path::{Path, PathBuf},
	sync::Arc,
};

use jiff::Timestamp;
use miette::{IntoDiagnostic as _, Result, WrapErr as _};
use tracing::debug;
use uuid::Uuid;

use super::{
	paths::{self, AuditFile},
	record::{ContextRecord, QuerySource, Record, RecordKind, SEPARATOR, TERMINATOR},
	tailscale::TailscalePeer,
};

/// How much of a segment is read at a time when walking it backwards.
const REVERSE_CHUNK: usize = 64 * 1024;

/// Most bytes taken as one record before the reader gives up on it.
///
/// A record is held whole to be parsed, so without a bound a file with no
/// separator in it is an unbounded allocation — and behind a day file's
/// decompression, a few compressible kilobytes on disk are enough to ask for it.
/// The read API is pointed at stores copied off other machines, so the bound has
/// to be here rather than assumed of the input. It sits far above any statement
/// an operator would type or paste.
const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;

/// A segment whose file name names one session and whose records name another.
///
/// The name is outside the hash chain and the record inside it, so the two
/// disagreeing means the file was renamed after it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed {
	pub file: PathBuf,
	/// The session the file name claims.
	pub names: Uuid,
	/// The session its records say wrote them.
	pub records: Uuid,
}

/// A stretch of bytes a reader could not parse as a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
	pub file: PathBuf,
	pub at: u64,
	pub bytes: usize,
}

/// One item out of a framed file: a record, bytes that were not one, or a read
/// that could go no further.
#[derive(Debug, Clone)]
pub enum FramedItem {
	Record {
		record: Record,
		json: String,
		at: u64,
	},
	Skipped {
		at: u64,
		bytes: usize,
	},
	/// The file stopped being readable partway through.
	///
	/// Unlike unparsable bytes, this says nothing about what the rest of the
	/// file holds, so a caller that is about to replace the file must not treat
	/// it as the end of one.
	Failed {
		at: u64,
		error: String,
	},
}

impl FramedItem {
	pub fn into_record(self) -> Option<Record> {
		match self {
			Self::Record { record, .. } => Some(record),
			Self::Skipped { .. } | Self::Failed { .. } => None,
		}
	}
}

/// Splits a JSON text sequence into records.
///
/// Bytes that cannot be parsed as a record are reported and the reader resumes
/// at the next separator, so damage is confined to the record it lands in: a
/// half-written record cannot swallow the records after it, and cannot be
/// mistaken for the end of the file.
pub struct FramedReader<R> {
	inner: R,
	offset: u64,
	started: bool,
	failed: bool,
}

impl<R: BufRead> FramedReader<R> {
	pub fn new(inner: R) -> Self {
		Self {
			inner,
			offset: 0,
			started: false,
			failed: false,
		}
	}
}

impl<R: BufRead> FramedReader<R> {
	/// Read up to the next separator, stopping a little past the bound so the
	/// caller can tell that it was exceeded.
	fn take_record(&mut self, buf: &mut Vec<u8>) -> std::io::Result<usize> {
		let mut read = 0;
		loop {
			let taken = self.inner.read_until(SEPARATOR, buf)?;
			read += taken;
			if taken == 0 || buf.last() == Some(&SEPARATOR) || buf.len() > MAX_RECORD_BYTES {
				return Ok(read);
			}
		}
	}
}

impl<R: BufRead> Iterator for FramedReader<R> {
	type Item = FramedItem;

	fn next(&mut self) -> Option<FramedItem> {
		if self.failed {
			return None;
		}

		loop {
			let mut buf = Vec::new();
			let read = match self.take_record(&mut buf) {
				Ok(0) => return None,
				Ok(read) => read,
				Err(err) => {
					debug!(?err, "reading audit file");
					self.failed = true;
					return Some(FramedItem::Failed {
						at: self.offset,
						error: err.to_string(),
					});
				}
			};

			// More than one record's worth of bytes with no separator among
			// them: skipped, and the reader picks up at the next separator, the
			// same treatment bytes that will not parse already get.
			if buf.len() > MAX_RECORD_BYTES {
				let at = self.offset;
				self.offset += read as u64;
				self.started = true;
				return Some(FramedItem::Skipped {
					at,
					bytes: buf.len(),
				});
			}

			let at = self.offset;
			self.offset += read as u64;

			if buf.last() == Some(&SEPARATOR) {
				buf.pop();
			}

			// Anything before the first separator is not a record: a well-formed
			// file starts with one, so this is normally empty.
			if !self.started {
				self.started = true;
				if buf.is_empty() {
					continue;
				}
				return Some(FramedItem::Skipped {
					at,
					bytes: buf.len(),
				});
			}

			if buf.last() == Some(&TERMINATOR) {
				buf.pop();
			}
			if buf.is_empty() {
				continue;
			}

			return Some(parse(buf, at));
		}
	}
}

fn parse(buf: Vec<u8>, at: u64) -> FramedItem {
	let bytes = buf.len();
	match String::from_utf8(buf) {
		Ok(json) => match serde_json::from_str::<Record>(&json) {
			Ok(record) => FramedItem::Record { record, json, at },
			Err(_) => FramedItem::Skipped { at, bytes },
		},
		Err(_) => FramedItem::Skipped { at, bytes },
	}
}

/// Walks a plain segment backwards, newest record first.
///
/// Used to build the recall set, where reading from the end is what keeps
/// startup flat no matter how busy the user has been.
pub struct ReverseFramedReader {
	file: File,
	remaining: u64,
	tail: Vec<u8>,
	ready: VecDeque<FramedItem>,
	done: bool,
}

impl ReverseFramedReader {
	pub fn open(path: &Path) -> Result<Self> {
		let file = File::open(path)
			.into_diagnostic()
			.wrap_err_with(|| format!("opening {}", path.display()))?;
		let remaining = file.metadata().into_diagnostic()?.len();
		Ok(Self {
			file,
			remaining,
			tail: Vec::new(),
			ready: VecDeque::new(),
			done: false,
		})
	}

	/// Read one chunk further back and split off whatever records it completes.
	fn fill(&mut self) -> Result<()> {
		// Compared as u64 and narrowed after: the other order truncates a large
		// length on a 32-bit target, and a chunk of zero would never make
		// progress.
		let take = self.remaining.min(REVERSE_CHUNK as u64) as usize;
		let start = self.remaining - take as u64;

		let mut chunk = vec![0u8; take];
		self.file.seek(SeekFrom::Start(start)).into_diagnostic()?;
		self.file.read_exact(&mut chunk).into_diagnostic()?;
		self.remaining = start;

		chunk.append(&mut self.tail);
		self.tail = chunk;

		while let Some(index) = self.tail.iter().rposition(|byte| *byte == SEPARATOR) {
			let mut record = self.tail.split_off(index);
			// `split_off` leaves the separator at the head of the tail piece.
			record.remove(0);
			if record.last() == Some(&TERMINATOR) {
				record.pop();
			}
			if !record.is_empty() {
				let at = start + index as u64;
				self.ready.push_back(parse(record, at));
			}
			self.tail.truncate(index);
		}

		// The same bound as reading forwards: bytes accumulating with no separator
		// among them are not a record anyone wrote, and holding them all to find
		// that out is what a crafted file would ask for.
		if self.tail.len() > MAX_RECORD_BYTES {
			self.ready.push_back(FramedItem::Skipped {
				at: start,
				bytes: self.tail.len(),
			});
			self.tail.clear();
		}

		if self.remaining == 0 {
			self.done = true;
			// Bytes before the first separator are not a record.
			if !self.tail.is_empty() {
				self.ready.push_back(FramedItem::Skipped {
					at: 0,
					bytes: self.tail.len(),
				});
				self.tail.clear();
			}
		}

		Ok(())
	}
}

impl Iterator for ReverseFramedReader {
	type Item = FramedItem;

	fn next(&mut self) -> Option<FramedItem> {
		loop {
			if let Some(item) = self.ready.pop_front() {
				return Some(item);
			}
			if self.done {
				return None;
			}
			if let Err(err) = self.fill() {
				debug!(?err, "reading audit segment backwards");
				self.done = true;
				return Some(FramedItem::Failed {
					at: self.remaining,
					error: err.to_string(),
				});
			}
		}
	}
}

/// Open any audit file as a forward stream of framed items.
pub fn open(path: &Path, kind: AuditFile) -> Result<Box<dyn Iterator<Item = FramedItem>>> {
	let file = File::open(path)
		.into_diagnostic()
		.wrap_err_with(|| format!("opening {}", path.display()))?;

	Ok(match kind {
		AuditFile::Segment { .. } => Box::new(FramedReader::new(BufReader::new(file))),
		// A day file wraps the same framed records, so the same record reader
		// runs over both.
		AuditFile::DayFile { .. } => {
			let decoder = zstd::Decoder::new(file)
				.into_diagnostic()
				.wrap_err_with(|| format!("decompressing {}", path.display()))?;
			Box::new(FramedReader::new(BufReader::new(decoder)))
		}
	})
}

/// A record as stored, with the session it was attributed to.
#[derive(Debug, Clone)]
pub struct Stored {
	pub record: Record,
	/// The record's JSON text, exactly as written, which is what hashes.
	pub json: String,
	pub hash: String,
	/// The session that wrote it, where the log lets a reader work that out.
	pub instance: Option<Uuid>,
}

/// A query record with the context in force at it, so a caller needs no state.
#[derive(Debug, Clone)]
pub struct Entry {
	pub ts: Timestamp,
	pub seq: u64,
	pub instance: Option<Uuid>,
	pub query: String,
	pub source: QuerySource,
	pub sys_user: String,
	pub db_user: String,
	pub writemode: bool,
	pub ots: Option<String>,
	pub tailscale: Vec<TailscalePeer>,
}

/// Attributes records to the sessions that wrote them.
///
/// Only context records name their session. Every other record is attributed by
/// following its `prev` back to the chain it extends, which is what lets a day
/// file interleave several sessions and still be read apart.
#[derive(Debug, Default)]
struct Threads {
	/// Hash of each session's current chain head.
	head: HashMap<String, Uuid>,
	context: HashMap<Uuid, Arc<Record>>,
}

impl Threads {
	/// `named` is the session the file itself names, where it names one.
	///
	/// Returns the session, and whether the file's name disagrees with what the
	/// record itself says.
	fn attribute(
		&mut self,
		record: &Record,
		hash: &str,
		named: Option<Uuid>,
	) -> (Option<Uuid>, bool) {
		// A segment is written by exactly one session, so its name settles the
		// question outright: attribution does not depend on the chain, and a
		// record whose predecessor was altered or removed is still known to
		// belong to the session whose chain it broke.
		// A file's name is outside the hash chain; the session identity in a
		// context record is inside it. Where the two disagree the file has been
		// renamed, and the record is believed over the name — silently taking
		// the name would let a `mv` re-attribute every statement in a segment
		// from one operator's session to another's.
		let renamed = match (named, record.instance()) {
			(Some(named), Some(recorded)) => named != recorded,
			_ => false,
		};

		// In order of what is covered by the hash chain: the identity written
		// into the record, then the chain it extends, then — only for a record
		// whose predecessor is not in the log at all — the file's name. Taking
		// the name first would let a rename hand one operator's statements to
		// another session; taking it last still attributes the records after a
		// tampered one, which is what it is there for.
		let instance = record
			.instance()
			.or_else(|| self.head.get(&record.prev).copied())
			.or(named);

		if !record.prev.is_empty() {
			self.head.remove(&record.prev);
		}
		if let Some(instance) = instance {
			self.head.insert(hash.to_owned(), instance);
			if matches!(record.kind, RecordKind::Context(_)) {
				self.context.insert(instance, Arc::new(record.clone()));
			}
		}

		(instance, renamed)
	}
}

/// Restricts a read to a span of time.
#[derive(Debug, Clone, Copy, Default)]
pub struct Range {
	pub since: Option<Timestamp>,
	pub until: Option<Timestamp>,
}

impl Range {
	fn holds(&self, ts: Timestamp) -> bool {
		self.since.is_none_or(|since| ts >= since) && self.until.is_none_or(|until| ts <= until)
	}

	/// Whether a whole day could hold anything in range, so a file outside it is
	/// never opened.
	fn could_hold(&self, kind: AuditFile) -> bool {
		let date = kind.date();
		self.since
			.is_none_or(|since| date >= super::writer::date_of(since))
			&& self
				.until
				.is_none_or(|until| date <= super::writer::date_of(until))
	}
}

struct Source {
	iter: Box<dyn Iterator<Item = FramedItem>>,
	origin: PathBuf,
	/// The session a segment belongs to, which its name gives directly. A day
	/// file interleaves several sessions and so names none.
	instance: Option<Uuid>,
	head: Option<(Record, String)>,
}

impl Source {
	fn fill(&mut self, skipped: &mut Vec<Skipped>) {
		while self.head.is_none() {
			match self.iter.next() {
				None => break,
				Some(FramedItem::Record { record, json, .. }) => self.head = Some((record, json)),
				Some(FramedItem::Skipped { at, bytes }) => skipped.push(Skipped {
					file: self.origin.clone(),
					at,
					bytes,
				}),
				Some(FramedItem::Failed { at, error }) => {
					debug!(origin = ?self.origin, at, %error, "audit file stopped being readable");
				}
			}
		}
	}
}

/// Reads a whole audit directory as one time-ordered stream of records.
pub struct Reader {
	/// Files not yet opened, newest day last, so they can be popped in order.
	waiting: Vec<(PathBuf, AuditFile)>,
	/// Segments whose name disagrees with the session recorded inside them.
	renamed: Vec<Renamed>,
	sources: Vec<Source>,
	queue: BinaryHeap<Reverse<(Timestamp, u64, usize)>>,
	threads: Threads,
	skipped: Vec<Skipped>,
	range: Range,
	/// Records already emitted at the timestamp being read now. Copies left by
	/// an interrupted compaction are byte-identical and so share a timestamp,
	/// which keeps this window small.
	at: Option<Timestamp>,
	seen: HashSet<String>,
}

impl Reader {
	/// Open every segment and day file in a directory.
	pub fn open(dir: &Path) -> Result<Self> {
		Self::open_range(dir, Range::default())
	}

	pub fn open_range(dir: &Path, range: Range) -> Result<Self> {
		// Oldest day last, so the newest is at the bottom of the stack and days
		// come off it in order.
		let mut waiting: Vec<(PathBuf, AuditFile)> = paths::list(dir)?
			.into_iter()
			.filter(|(_, kind)| range.could_hold(*kind))
			.collect();
		waiting.reverse();

		let mut reader = Self {
			waiting,
			renamed: Vec::new(),
			sources: Vec::new(),
			queue: BinaryHeap::new(),
			threads: Threads::default(),
			skipped: Vec::new(),
			range,
			at: None,
			seen: HashSet::new(),
		};
		reader.open_next_day();
		Ok(reader)
	}

	/// Open the files covering the next day that has any.
	///
	/// Only one day is open at a time. Every record in a file dated D falls on
	/// day D — no segment spans a date boundary and a day file holds one day —
	/// so a day can be merged to completion before the next is opened. At the
	/// twelve-month retention period that is the difference between a handful of
	/// open files and a year of them, each day file carrying a decompression
	/// context of its own.
	fn open_next_day(&mut self) {
		let Some((_, next)) = self.waiting.last() else {
			return;
		};
		let date = next.date();

		while let Some((path, kind)) = self.waiting.last() {
			if kind.date() != date {
				break;
			}
			let (path, kind) = (path.clone(), *kind);
			self.waiting.pop();

			match open(&path, kind) {
				Ok(iter) => {
					let index = self.sources.len();
					self.sources.push(Source {
						iter,
						origin: path,
						instance: kind.instance(),
						head: None,
					});
					self.advance(index);
				}
				// One unreadable file must not stop the rest of the log being
				// read: an auditor gets what survives, and hears about the rest.
				Err(err) => debug!(?err, ?path, "skipping unreadable audit file"),
			}
		}
	}

	fn advance(&mut self, index: usize) {
		let mut skipped = std::mem::take(&mut self.skipped);
		self.sources[index].fill(&mut skipped);
		self.skipped = skipped;

		if let Some((record, _)) = &self.sources[index].head {
			self.queue.push(Reverse((record.ts, record.seq, index)));
		}
	}

	/// The unparsable stretches met so far.
	pub fn skipped(&self) -> &[Skipped] {
		&self.skipped
	}

	/// Segments whose name disagrees with the session recorded inside them.
	pub fn renamed(&self) -> &[Renamed] {
		&self.renamed
	}

	/// The context in force for a session at the point the read has reached.
	pub fn context_of(&self, instance: Uuid) -> Option<&ContextRecord> {
		self.context_record_of(instance).map(|record| {
			let RecordKind::Context(context) = &record.kind else {
				unreachable!("only context records are kept as context")
			};
			context
		})
	}

	/// The context record in force for a session at the point the read has
	/// reached, whole, so a caller can write it out as it stands.
	pub fn context_record_of(&self, instance: Uuid) -> Option<&Arc<Record>> {
		self.threads.context.get(&instance)
	}

	/// Flat entries: every query record with the context in force at it.
	pub fn entries(self) -> impl Iterator<Item = Entry> {
		Entries { reader: self }
	}
}

impl Iterator for Reader {
	type Item = Stored;

	fn next(&mut self) -> Option<Stored> {
		loop {
			if self.queue.is_empty() {
				if self.waiting.is_empty() {
					return None;
				}
				// The day just finished; the next one can be opened now, and the
				// files behind it closed.
				self.sources.clear();
				self.open_next_day();
				continue;
			}

			let Reverse((ts, _, index)) = self.queue.pop()?;
			let (record, json) = self.sources[index].head.take()?;
			self.advance(index);

			let hash = super::record::hash(&json);

			// Copies of one record are byte-identical and share a timestamp, so
			// a window over equal timestamps is enough to fold them together.
			if self.at != Some(ts) {
				self.at = Some(ts);
				self.seen.clear();
			}
			if !self.seen.insert(hash.clone()) {
				continue;
			}

			// Attribution runs over every record, in range or not, because a
			// context record outside the range still names the session that the
			// records inside it belong to.
			let named = self.sources[index].instance;
			let (instance, renamed) = self.threads.attribute(&record, &hash, named);
			if renamed {
				self.renamed.push(Renamed {
					file: self.sources[index].origin.clone(),
					names: named.expect("a name to disagree with"),
					records: instance.expect("a record that said so"),
				});
			}

			if !self.range.holds(ts) {
				continue;
			}

			return Some(Stored {
				record,
				json,
				hash,
				instance,
			});
		}
	}
}

struct Entries {
	reader: Reader,
}

impl Iterator for Entries {
	type Item = Entry;

	fn next(&mut self) -> Option<Entry> {
		loop {
			let stored = self.reader.next()?;
			let RecordKind::Query(query) = &stored.record.kind else {
				continue;
			};

			let context = stored
				.instance
				.and_then(|instance| self.reader.context_of(instance));

			return Some(Entry {
				ts: stored.record.ts,
				seq: stored.record.seq,
				instance: stored.instance,
				query: query.query.clone(),
				source: query.source.clone(),
				sys_user: context.map(|c| c.sys_user.clone()).unwrap_or_default(),
				db_user: context.map(|c| c.db_user.clone()).unwrap_or_default(),
				writemode: context.is_some_and(|c| c.writemode),
				ots: context.and_then(|c| c.ots.clone()),
				tailscale: context.map(|c| c.tailscale.clone()).unwrap_or_default(),
			});
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::audit::{
		record::{FORMAT_VERSION, QueryRecord, frame},
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

	fn framed(records: &[Record]) -> Vec<u8> {
		records
			.iter()
			.flat_map(|r| frame(&r.to_json().unwrap()))
			.collect()
	}

	fn record(seq: u64, query: &str) -> Record {
		Record {
			v: FORMAT_VERSION,
			seq,
			ts: Timestamp::UNIX_EPOCH + jiff::SignedDuration::from_secs(seq as i64),
			prev: String::new(),
			kind: RecordKind::Query(QueryRecord {
				query: query.into(),
				source: QuerySource::Typed,
			}),
		}
	}

	fn read_all(bytes: &[u8]) -> Vec<FramedItem> {
		FramedReader::new(BufReader::new(bytes)).collect()
	}

	#[test]
	fn framing_round_trips() {
		let records = vec![record(0, "select 1;"), record(1, "select 2;")];
		let items = read_all(&framed(&records));
		let read: Vec<_> = items.into_iter().filter_map(|i| i.into_record()).collect();
		assert_eq!(read, records);
	}

	#[test]
	fn an_empty_file_reads_as_no_records() {
		assert!(read_all(b"").is_empty());
	}

	#[test]
	fn a_torn_final_record_is_reported_and_the_rest_survive() {
		let mut bytes = framed(&[record(0, "select 1;"), record(1, "select 2;")]);
		bytes.extend_from_slice(&[SEPARATOR]);
		bytes.extend_from_slice(br#"{"v":1,"seq":2,"ts":"1970-01-0"#);

		let items = read_all(&bytes);
		assert_eq!(items.len(), 3);
		assert!(matches!(items[2], FramedItem::Skipped { .. }));

		let read: Vec<_> = items.into_iter().filter_map(|i| i.into_record()).collect();
		assert_eq!(read.len(), 2);
	}

	#[test]
	fn damage_in_the_middle_does_not_swallow_the_records_after_it() {
		let mut bytes = framed(&[record(0, "before;")]);
		bytes.push(SEPARATOR);
		bytes.extend_from_slice(b"not json at all");
		bytes.push(TERMINATOR);
		bytes.extend_from_slice(&framed(&[record(2, "after;")]));

		let items = read_all(&bytes);
		let queries: Vec<_> = items
			.iter()
			.cloned()
			.filter_map(|i| i.into_record())
			.filter_map(|r| match r.kind {
				RecordKind::Query(q) => Some(q.query),
				_ => None,
			})
			.collect();
		assert_eq!(queries, vec!["before;", "after;"]);
		assert_eq!(
			items
				.iter()
				.filter(|i| matches!(i, FramedItem::Skipped { .. }))
				.count(),
			1
		);
	}

	#[test]
	fn junk_before_the_first_separator_is_reported() {
		let mut bytes = b"leading junk".to_vec();
		bytes.extend_from_slice(&framed(&[record(0, "select 1;")]));

		let items = read_all(&bytes);
		assert!(matches!(items[0], FramedItem::Skipped { at: 0, bytes: 12 }));
		assert_eq!(items.len(), 2);
	}

	#[test]
	fn a_record_missing_its_newline_still_parses() {
		let json = record(0, "select 1;").to_json().unwrap();
		let mut bytes = vec![SEPARATOR];
		bytes.extend_from_slice(json.as_bytes());

		let items = read_all(&bytes);
		assert_eq!(items.len(), 1);
		let FramedItem::Record { json: read, .. } = &items[0] else {
			panic!("expected a record");
		};
		assert_eq!(*read, json);
	}

	#[test]
	fn a_run_of_bytes_with_no_separator_is_skipped_rather_than_held() {
		// What a crafted day file amounts to once decompressed: highly
		// compressible bytes with no separator among them. Held whole it is an
		// unbounded allocation, so it is skipped like anything else that will
		// not parse.
		let mut bytes = vec![b'x'; MAX_RECORD_BYTES + 1024];
		bytes.extend_from_slice(&framed(&[record(0, "after the junk;")]));

		let items = read_all(&bytes);
		assert!(matches!(items[0], FramedItem::Skipped { .. }));

		let queries: Vec<_> = items
			.into_iter()
			.filter_map(|i| i.into_record())
			.filter_map(|r| match r.kind {
				RecordKind::Query(q) => Some(q.query),
				_ => None,
			})
			.collect();
		assert_eq!(queries, vec!["after the junk;"]);
	}

	#[test]
	fn reading_backwards_yields_the_newest_record_first() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("segment");
		let records: Vec<_> = (0..200)
			.map(|i| record(i, &format!("select {i};")))
			.collect();
		std::fs::write(&path, framed(&records)).unwrap();

		let read: Vec<_> = ReverseFramedReader::open(&path)
			.unwrap()
			.filter_map(|i| i.into_record())
			.collect();

		let mut expected = records;
		expected.reverse();
		assert_eq!(read, expected);
	}

	#[test]
	fn reading_backwards_handles_a_record_larger_than_a_chunk() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("segment");
		let big = "x".repeat(REVERSE_CHUNK * 3);
		let records = vec![
			record(0, "small;"),
			record(1, &big),
			record(2, "also small;"),
		];
		std::fs::write(&path, framed(&records)).unwrap();

		let read: Vec<_> = ReverseFramedReader::open(&path)
			.unwrap()
			.filter_map(|i| i.into_record())
			.collect();
		let mut expected = records;
		expected.reverse();
		assert_eq!(read, expected);
	}

	#[test]
	fn reading_backwards_matches_reading_forwards() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		for i in 0..50 {
			writer.query(&context(), format!("select {i};"), QuerySource::Typed);
		}
		drop(writer);

		let path = paths::list(dir.path()).unwrap()[0].0.clone();
		let forwards: Vec<_> = FramedReader::new(BufReader::new(File::open(&path).unwrap()))
			.filter_map(|i| i.into_record())
			.collect();
		let mut backwards: Vec<_> = ReverseFramedReader::open(&path)
			.unwrap()
			.filter_map(|i| i.into_record())
			.collect();
		backwards.reverse();
		assert_eq!(forwards, backwards);
	}

	#[test]
	fn the_merge_orders_concurrent_sessions_by_time() {
		let dir = tempfile::tempdir().unwrap();
		let mut one = Writer::new(dir.path());
		let mut two = Writer::new(dir.path());
		one.query(&context(), "one a;".into(), QuerySource::Typed);
		two.query(&context(), "two a;".into(), QuerySource::Typed);
		one.query(&context(), "one b;".into(), QuerySource::Typed);
		two.query(&context(), "two b;".into(), QuerySource::Typed);
		drop(one);
		drop(two);

		let queries: Vec<_> = Reader::open(dir.path())
			.unwrap()
			.entries()
			.map(|e| e.query)
			.collect();
		assert_eq!(queries, vec!["one a;", "two a;", "one b;", "two b;"]);
	}

	#[test]
	fn every_entry_carries_the_context_of_its_own_session() {
		let dir = tempfile::tempdir().unwrap();
		let mut one = Writer::new(dir.path());
		let mut two = Writer::new(dir.path());

		let alice = Context {
			sys_user: "alice".into(),
			db_user: "tamanu".into(),
			writemode: false,
			ots: None,
		};
		let bob = Context {
			sys_user: "bob".into(),
			db_user: "tamanu".into(),
			writemode: true,
			ots: Some("Carol".into()),
		};

		one.query(&alice, "from alice;".into(), QuerySource::Typed);
		two.query(&bob, "from bob;".into(), QuerySource::Typed);
		one.query(&alice, "alice again;".into(), QuerySource::Typed);
		drop(one);
		drop(two);

		let entries: Vec<_> = Reader::open(dir.path()).unwrap().entries().collect();
		let by_query = |q: &str| entries.iter().find(|e| e.query == q).unwrap().clone();

		assert_eq!(by_query("from alice;").sys_user, "alice");
		assert!(!by_query("from alice;").writemode);
		assert_eq!(by_query("from bob;").sys_user, "bob");
		assert!(by_query("from bob;").writemode);
		assert_eq!(by_query("from bob;").ots.as_deref(), Some("Carol"));
		assert_eq!(by_query("alice again;").sys_user, "alice");
	}

	#[test]
	fn records_are_attributed_to_their_session() {
		let dir = tempfile::tempdir().unwrap();
		let mut one = Writer::new(dir.path());
		let mut two = Writer::new(dir.path());
		let (a, b) = (one.instance(), two.instance());

		one.query(&context(), "one;".into(), QuerySource::Typed);
		two.query(&context(), "two;".into(), QuerySource::Typed);
		drop(one);
		drop(two);

		for stored in Reader::open(dir.path()).unwrap() {
			assert!(
				stored.instance == Some(a) || stored.instance == Some(b),
				"every record is attributed"
			);
		}
	}

	#[test]
	fn a_time_range_narrows_the_stream() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "first;".into(), QuerySource::Typed);
		let cut = Timestamp::now();
		std::thread::sleep(std::time::Duration::from_millis(5));
		writer.query(&context(), "second;".into(), QuerySource::Typed);
		drop(writer);

		let entries: Vec<_> = Reader::open_range(
			dir.path(),
			Range {
				since: Some(cut),
				until: None,
			},
		)
		.unwrap()
		.entries()
		.map(|e| e.query)
		.collect();
		assert_eq!(entries, vec!["second;"]);
	}

	#[test]
	fn an_entry_outside_the_range_still_gets_its_context() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "first;".into(), QuerySource::Typed);
		let cut = Timestamp::now();
		std::thread::sleep(std::time::Duration::from_millis(5));
		writer.query(&context(), "second;".into(), QuerySource::Typed);
		drop(writer);

		// The context record is before the cut, so attribution has to survive
		// the range filter.
		let entry = Reader::open_range(
			dir.path(),
			Range {
				since: Some(cut),
				until: None,
			},
		)
		.unwrap()
		.entries()
		.next()
		.unwrap();
		assert_eq!(entry.sys_user, "felix");
		assert_eq!(entry.db_user, "tamanu");
	}

	#[test]
	fn an_empty_directory_reads_as_an_empty_log() {
		let dir = tempfile::tempdir().unwrap();
		assert_eq!(Reader::open(dir.path()).unwrap().count(), 0);
	}
}
