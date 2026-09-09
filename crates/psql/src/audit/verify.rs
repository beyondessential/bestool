//! Chain verification.
//!
//! The chain shows that a session's records have not been edited, reordered or
//! removed since they were written. Verification follows each session's records
//! across the segments and day files that hold them, and tells an incomplete log
//! apart from an altered one.
//!
//! spec: AUD-STO, AUD-API

use std::{
	collections::{BTreeMap, HashMap},
	fmt,
	path::Path,
};

use jiff::Timestamp;
use miette::Result;
use uuid::Uuid;

use super::{
	read::{Range, Reader, Skipped},
	record::{GapRecord, Record, RecordKind},
	writer::{BACKLOG_BYTES, BACKLOG_RECORDS},
};

/// Roughly how much memory a record held for reordering costs.
fn json_weight(record: &Record) -> usize {
	match &record.kind {
		RecordKind::Query(query) => query.query.len(),
		_ => 0,
	}
}

/// How many out-of-order records are held per session before the chain is
/// called broken. Records only ever arrive out of order by as much as a
/// session's own held backlog, so this is generous.
const REORDER_LIMIT: usize = BACKLOG_RECORDS * 10;

/// How many bytes of out-of-order records are held per session.
///
/// Statement text is bounded only by the writer's own backlog, so a count on its
/// own is not a bound on memory: a session whose chain breaks early — the very
/// case verification exists to diagnose — would otherwise park whole records
/// until the log ran out, once per session in the directory.
const REORDER_BYTES: usize = BACKLOG_BYTES;

/// Where a session's chain first stopped holding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Break {
	pub seq: u64,
	pub ts: Timestamp,
	pub reason: BreakReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreakReason {
	/// The record does not chain onto the one before it.
	Unchained { expected: String, found: String },
	/// A sequence number is missing without a gap record accounting for it.
	Missing { expected: u64 },
}

impl fmt::Display for BreakReason {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Unchained { expected, found } => {
				let found = if found.is_empty() { "nothing" } else { found };
				write!(f, "chains onto {found}, expected {expected}")
			}
			Self::Missing { expected } => {
				write!(f, "sequence number {expected} is unaccounted for")
			}
		}
	}
}

/// What verification found for one session.
#[derive(Debug, Clone)]
pub struct ChainReport {
	pub instance: Uuid,
	pub records: u64,
	/// The first record at which the chain breaks.
	pub broken_at: Option<Break>,
	/// Every gap record passed, with the sequence number it took.
	pub gaps: Vec<(u64, GapRecord)>,
	/// Whether the oldest record kept could not be checked against what came
	/// before it, because retention has deleted it.
	pub truncated_start: bool,
	/// The session's current chain head, which is what an off-box witness would
	/// publish.
	pub head: String,
}

impl ChainReport {
	pub fn holds(&self) -> bool {
		self.broken_at.is_none()
	}

	/// How many records the gaps say were lost.
	pub fn lost(&self) -> u64 {
		self.gaps.iter().map(|(_, gap)| gap.lost).sum()
	}
}

/// What verification found across the whole log.
#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
	pub sessions: Vec<ChainReport>,
	/// Stretches of bytes that were not records.
	pub skipped: Vec<Skipped>,
	/// Records that could not be tied to any session, because the chain they
	/// extend is not in the log.
	pub unattributed: u64,
}

impl VerifyReport {
	/// Whether every session's chain holds.
	///
	/// Gap records and unparsable bytes are reported but do not fail a log: they
	/// say where it is incomplete, which is not the same as altered.
	///
	/// A record belonging to no session in the log does fail it. Every segment
	/// names its session, and every day file carries a context record for each
	/// session in it, so a record that cannot be tied to one has been put there
	/// by something other than a session writing its own segment.
	pub fn holds(&self) -> bool {
		self.unattributed == 0 && self.sessions.iter().all(ChainReport::holds)
	}

	/// The current chain head of every session.
	pub fn heads(&self) -> impl Iterator<Item = (Uuid, &str)> {
		self.sessions
			.iter()
			.map(|session| (session.instance, session.head.as_str()))
	}
}

/// One session's chain, as far as it has been followed.
#[derive(Debug)]
struct Chain {
	records: u64,
	head: String,
	broken_at: Option<Break>,
	gaps: Vec<(u64, GapRecord)>,
	truncated_start: bool,
	/// Every sequence number below this is accounted for, by a record or by a
	/// gap covering it.
	covered_to: u64,
	/// Runs of sequence numbers accounted for above `covered_to`, keyed by the
	/// number each run starts at.
	ahead: BTreeMap<u64, u64>,
	/// The earliest record met whose number sat above a hole, kept so the hole
	/// can be reported against something.
	first_ahead: Option<(u64, Timestamp)>,
	/// Records whose predecessor has not been seen yet, keyed by the hash they
	/// chain onto. A held backlog is written in sequence order but its records
	/// were made before the gap record that precedes them, so a reader can meet
	/// them slightly out of order.
	pending: HashMap<String, (Record, String)>,
	/// Bytes of JSON held in `pending`.
	pending_bytes: usize,
}

impl Chain {
	fn start(record: &Record, hash: &str) -> Self {
		// Only the first record a session ever writes has an empty `prev`.
		// Anything else means the records before it are no longer kept, which
		// is retention doing its job rather than the chain being broken.
		let truncated_start = !(record.prev.is_empty() && record.seq == 0);

		let mut chain = Self {
			records: 0,
			head: String::new(),
			broken_at: None,
			gaps: Vec::new(),
			truncated_start,
			// Where retention has removed the earlier records, the numbering
			// starts from the oldest one kept rather than from zero.
			covered_to: record.seq,
			ahead: BTreeMap::new(),
			first_ahead: None,
			pending: HashMap::new(),
			pending_bytes: 0,
		};
		chain.consume(record, hash);
		chain
	}

	fn consume(&mut self, record: &Record, hash: &str) {
		self.records += 1;

		// What this record accounts for. A gap accounts for every number through
		// the one it names; the number comes out of the file, so it is whatever
		// the file says. Verification runs over logs that may have been tampered
		// with and must report on them rather than fall over.
		let through = match &record.kind {
			RecordKind::Gap(gap) => {
				self.gaps.push((record.seq, gap.clone()));
				gap.through.max(record.seq)
			}
			_ => record.seq,
		};

		self.account(record.seq, through, record.ts);
		self.head = hash.to_owned();
	}

	/// Note that the numbers `from..=through` are accounted for.
	///
	/// Numbering is checked by coverage rather than by each record following on
	/// from the last, because a record can legitimately arrive out of numbering
	/// order: the context record behind a gap takes a fresh number but the
	/// timestamp of the gap, so it is read before the held records it precedes.
	fn account(&mut self, from: u64, through: u64, ts: Timestamp) {
		if through < self.covered_to {
			// Already accounted for, which a gap's range can cover.
			return;
		}

		if from > self.covered_to {
			if self.first_ahead.is_none() {
				self.first_ahead = Some((from, ts));
			}
			if self.ahead.len() < REORDER_LIMIT {
				let end = self.ahead.entry(from).or_insert(through);
				*end = (*end).max(through);
			}
		} else {
			self.covered_to = through.saturating_add(1);
		}

		// Take up any run that now follows on.
		while let Some((&start, &end)) = self.ahead.range(..=self.covered_to).next_back() {
			self.ahead.remove(&start);
			self.covered_to = self.covered_to.max(end.saturating_add(1));
		}
		if self.ahead.is_empty() {
			self.first_ahead = None;
		}
	}

	fn offer(&mut self, record: Record, hash: String) {
		if record.prev == self.head {
			self.consume(&record, &hash);
			self.drain();
			return;
		}

		if self.pending.len() >= REORDER_LIMIT || self.pending_bytes >= REORDER_BYTES {
			self.note_break(&record);
			return;
		}
		self.pending_bytes += hash.len() + json_weight(&record);
		self.pending.insert(record.prev.clone(), (record, hash));
	}

	/// Take up anything that was waiting on the record just consumed.
	fn drain(&mut self) {
		while let Some((record, hash)) = self.pending.remove(&self.head) {
			self.pending_bytes = self
				.pending_bytes
				.saturating_sub(hash.len() + json_weight(&record));
			self.consume(&record, &hash);
		}
	}

	fn note_break(&mut self, record: &Record) {
		if self.broken_at.is_none() {
			self.broken_at = Some(Break {
				seq: record.seq,
				ts: record.ts,
				reason: BreakReason::Unchained {
					expected: self.head.clone(),
					found: record.prev.clone(),
				},
			});
		}
		self.records += 1;
	}

	/// Anything still waiting once the log runs out never found its predecessor,
	/// and any number still unaccounted for is a hole in the numbering.
	fn finish(mut self, instance: Uuid) -> ChainReport {
		if let Some((seq, ts)) = self.first_ahead
			&& self.broken_at.is_none()
		{
			self.broken_at = Some(Break {
				seq,
				ts,
				reason: BreakReason::Missing {
					expected: self.covered_to,
				},
			});
		}

		if !self.pending.is_empty() {
			let orphan = self
				.pending
				.values()
				.min_by_key(|(record, _)| (record.seq, record.ts))
				.map(|(record, _)| record.clone());
			if let Some(orphan) = orphan {
				self.note_break(&orphan);
			}
			self.records += self.pending.len() as u64 - 1;
		}

		ChainReport {
			instance,
			records: self.records,
			broken_at: self.broken_at,
			gaps: self.gaps,
			truncated_start: self.truncated_start,
			head: self.head,
		}
	}
}

/// Verify every session's chain across a whole audit directory.
pub fn verify(dir: &Path) -> Result<VerifyReport> {
	verify_range(dir, Range::default())
}

pub fn verify_range(dir: &Path, range: Range) -> Result<VerifyReport> {
	let mut reader = Reader::open_range(dir, range)?;
	let mut chains: HashMap<Uuid, Chain> = HashMap::new();
	let mut order: Vec<Uuid> = Vec::new();
	let mut unattributed = 0;

	for stored in reader.by_ref() {
		let Some(instance) = stored.instance else {
			unattributed += 1;
			continue;
		};

		match chains.get_mut(&instance) {
			Some(chain) => chain.offer(stored.record, stored.hash),
			None => {
				order.push(instance);
				chains.insert(instance, Chain::start(&stored.record, &stored.hash));
			}
		}
	}

	let skipped = reader.skipped().to_vec();
	let sessions = order
		.into_iter()
		.filter_map(|instance| chains.remove(&instance).map(|chain| chain.finish(instance)))
		.collect();

	Ok(VerifyReport {
		sessions,
		skipped,
		unattributed,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::audit::{
		record::{QuerySource, frame},
		writer::{Context, Writer},
	};
	use jiff::Timestamp;

	fn context() -> Context {
		Context {
			sys_user: "felix".into(),
			db_user: "tamanu".into(),
			writemode: false,
			ots: None,
		}
	}

	fn write_session(dir: &Path, queries: usize) -> Uuid {
		let mut writer = Writer::new(dir);
		let instance = writer.instance();
		for i in 0..queries {
			writer.query(&context(), format!("select {i};"), QuerySource::Typed);
		}
		instance
	}

	fn segment_of(dir: &Path, instance: Uuid) -> std::path::PathBuf {
		crate::audit::paths::list(dir)
			.unwrap()
			.into_iter()
			.find(|(_, kind)| kind.instance() == Some(instance))
			.map(|(path, _)| path)
			.unwrap()
	}

	#[test]
	fn a_clean_session_verifies() {
		let dir = tempfile::tempdir().unwrap();
		let instance = write_session(dir.path(), 5);

		let report = verify(dir.path()).unwrap();
		assert!(report.holds());
		assert_eq!(report.sessions.len(), 1);
		assert_eq!(report.sessions[0].instance, instance);
		assert!(!report.sessions[0].truncated_start);
		assert!(report.sessions[0].gaps.is_empty());
		assert!(!report.sessions[0].head.is_empty());
		assert_eq!(report.unattributed, 0);
	}

	#[test]
	fn concurrent_sessions_each_verify() {
		let dir = tempfile::tempdir().unwrap();
		let mut one = Writer::new(dir.path());
		let mut two = Writer::new(dir.path());
		for i in 0..10 {
			one.query(&context(), format!("one {i};"), QuerySource::Typed);
			two.query(&context(), format!("two {i};"), QuerySource::Typed);
		}
		drop(one);
		drop(two);

		let report = verify(dir.path()).unwrap();
		assert_eq!(report.sessions.len(), 2);
		assert!(report.holds());
	}

	#[test]
	fn altering_a_record_in_place_breaks_the_chain() {
		let dir = tempfile::tempdir().unwrap();
		let instance = write_session(dir.path(), 5);
		let path = segment_of(dir.path(), instance);

		let text = std::fs::read_to_string(&path).unwrap();
		std::fs::write(&path, text.replace("select 2;", "select 9;")).unwrap();

		let report = verify(dir.path()).unwrap();
		assert!(!report.holds());
		let broken = report.sessions[0].broken_at.as_ref().unwrap();
		assert!(matches!(broken.reason, BreakReason::Unchained { .. }));
	}

	#[test]
	fn removing_a_record_breaks_the_chain() {
		let dir = tempfile::tempdir().unwrap();
		let instance = write_session(dir.path(), 5);
		let path = segment_of(dir.path(), instance);

		let bytes = std::fs::read(&path).unwrap();
		let text = String::from_utf8(bytes).unwrap();
		let kept: String = text
			.split('\u{1e}')
			.enumerate()
			.filter(|(i, _)| *i != 3)
			.map(|(_, part)| format!("\u{1e}{part}"))
			.collect::<String>()
			.replacen('\u{1e}', "", 1);
		std::fs::write(&path, kept).unwrap();

		let report = verify(dir.path()).unwrap();
		assert!(!report.holds());
	}

	/// Build a session whose records span three days, as a long-lived session
	/// rolling over at midnight does.
	fn session_over_three_days(dir: &Path) -> (Uuid, Vec<std::path::PathBuf>) {
		let mut writer = Writer::new(dir);
		let instance = writer.instance();
		writer.query(&context(), "day one;".into(), QuerySource::Typed);
		drop(writer);

		let today = crate::audit::writer::date_of(Timestamp::now());
		let first = dir.join(crate::audit::paths::segment_name(today, instance));
		let mut paths = vec![first.clone()];
		let mut prev_records = crate::audit::writer::read_records(&first);

		for day in 1..3 {
			let date = today.checked_add(jiff::Span::new().days(day)).unwrap();
			let previous = prev_records.last().unwrap().clone();
			let record = Record {
				v: crate::audit::record::FORMAT_VERSION,
				seq: previous.seq + 1,
				ts: previous.ts + jiff::SignedDuration::from_hours(24),
				prev: crate::audit::record::hash(&previous.to_json().unwrap()),
				kind: RecordKind::Query(crate::audit::record::QueryRecord {
					query: format!("day {};", day + 1),
					source: QuerySource::Typed,
				}),
			};
			let path = dir.join(crate::audit::paths::segment_name(date, instance));
			std::fs::write(&path, frame(&record.to_json().unwrap())).unwrap();
			prev_records = vec![record];
			paths.push(path);
		}

		(instance, paths)
	}

	#[test]
	fn removing_a_segment_from_the_middle_breaks_the_session_it_belonged_to() {
		let dir = tempfile::tempdir().unwrap();
		let (instance, paths) = session_over_three_days(dir.path());
		assert!(verify(dir.path()).unwrap().holds());

		std::fs::remove_file(&paths[1]).unwrap();

		let report = verify(dir.path()).unwrap();
		let session = report
			.sessions
			.iter()
			.find(|s| s.instance == instance)
			.unwrap();
		assert!(
			!session.holds(),
			"a whole segment cannot go missing unnoticed"
		);
	}

	#[test]
	fn losing_the_oldest_records_reads_as_retention_rather_than_tampering() {
		let dir = tempfile::tempdir().unwrap();
		let (instance, paths) = session_over_three_days(dir.path());

		// Retention deletes from the oldest end, which is exactly what removing
		// the earliest segment looks like: the oldest record kept can only be
		// reported as unverifiable, not as broken.
		std::fs::remove_file(&paths[0]).unwrap();

		let report = verify(dir.path()).unwrap();
		let session = report
			.sessions
			.iter()
			.find(|s| s.instance == instance)
			.unwrap();
		assert!(session.truncated_start);
		assert!(session.holds());
	}

	#[test]
	fn a_gap_is_reported_without_failing_the_chain() {
		let dir = tempfile::tempdir().unwrap();
		let store = dir.path().join("store");
		std::fs::write(&store, b"in the way").unwrap();

		let mut writer = Writer::new(&store);
		for i in 0..(crate::audit::writer::BACKLOG_RECORDS + 50) {
			writer.query(&context(), format!("lost {i};"), QuerySource::Typed);
		}
		std::fs::remove_file(&store).unwrap();
		writer.query(&context(), "after;".into(), QuerySource::Typed);
		drop(writer);

		let report = verify(&store).unwrap();
		assert_eq!(report.sessions.len(), 1);
		let session = &report.sessions[0];
		assert!(!session.gaps.is_empty(), "the gap is reported");
		assert!(session.lost() >= 50);
		assert!(
			session.holds(),
			"an incomplete log is not an altered one: {:?}",
			session.broken_at
		);
	}

	#[test]
	fn unparsable_bytes_are_reported() {
		let dir = tempfile::tempdir().unwrap();
		let instance = write_session(dir.path(), 3);
		let path = segment_of(dir.path(), instance);

		let mut bytes = std::fs::read(&path).unwrap();
		bytes.push(0x1E);
		bytes.extend_from_slice(b"half a record");
		std::fs::write(&path, bytes).unwrap();

		let report = verify(dir.path()).unwrap();
		assert_eq!(report.skipped.len(), 1);
		assert!(
			report.holds(),
			"a torn record does not break the records around it"
		);
	}

	#[test]
	fn a_crafted_gap_record_does_not_crash_the_verifier() {
		let dir = tempfile::tempdir().unwrap();
		let instance = write_session(dir.path(), 2);
		let path = segment_of(dir.path(), instance);

		// Verification is pointed at logs that may have been tampered with, so
		// numbers out of the file must be reported on, not trusted.
		let text = std::fs::read_to_string(&path).unwrap();
		let crafted = format!(
			r#"{{"v":1,"seq":1,"ts":"2026-09-08T00:00:00Z","prev":"","kind":"gap","lost":1,"through":{},"from":"2026-09-08T00:00:00Z","to":"2026-09-08T00:00:00Z"}}"#,
			u64::MAX
		);
		std::fs::write(&path, format!("{text}\u{1e}{crafted}\n")).unwrap();

		let report = verify(dir.path()).unwrap();
		assert!(!report.sessions.is_empty());
	}

	#[test]
	fn a_record_belonging_to_no_session_fails_the_log() {
		let dir = tempfile::tempdir().unwrap();
		let instance = write_session(dir.path(), 3);
		let path = segment_of(dir.path(), instance);

		// Splice a record into a day file, where nothing names the session it
		// claims to extend. In a segment the filename would give it away.
		let records = crate::audit::writer::read_records(&path);
		let spliced = Record {
			v: crate::audit::record::FORMAT_VERSION,
			seq: 99,
			ts: Timestamp::now(),
			prev: "0".repeat(64),
			kind: RecordKind::Query(crate::audit::record::QueryRecord {
				query: "spliced;".into(),
				source: QuerySource::Typed,
			}),
		};

		let day = dir.path().join(crate::audit::paths::day_file_name(
			crate::audit::writer::date_of(records[0].ts),
		));
		let mut bytes = Vec::new();
		for record in records.iter().chain(std::iter::once(&spliced)) {
			bytes.extend_from_slice(&frame(&record.to_json().unwrap()));
		}
		std::fs::write(&day, zstd::encode_all(&bytes[..], 3).unwrap()).unwrap();
		std::fs::remove_file(&path).unwrap();

		let report = verify(dir.path()).unwrap();
		assert_eq!(report.unattributed, 1);
		assert!(
			!report.holds(),
			"a record no session wrote cannot pass as one that was"
		);
	}

	#[test]
	fn an_empty_log_verifies() {
		let dir = tempfile::tempdir().unwrap();
		let report = verify(dir.path()).unwrap();
		assert!(report.holds());
		assert!(report.sessions.is_empty());
	}
}
