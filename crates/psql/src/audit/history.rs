//! Shell history, which is a view over the audit log rather than a store.
//!
//! spec: AUD-HIS

use std::{borrow::Cow, collections::VecDeque, path::Path};

use rustyline::history::{History, SearchDirection, SearchResult};
use tracing::debug;

use super::{
	paths::{self, AuditFile},
	read::{FramedItem, ReverseFramedReader, open},
	record::{ContextRecord, Record, RecordKind},
};

/// How much query text the recall set holds, which keeps startup cost and
/// memory flat no matter how busy the user has been.
pub const RECALL_BUDGET: usize = 4 * 1024 * 1024;

/// Statements longer than this are left out of the recall set entirely, so one
/// very large pasted statement never consumes the budget on its own. They stay
/// in the audit log.
pub const RECALL_CUTOFF: usize = 10 * 1024;

/// What a session recalls: the statements it can walk with up, down and search.
#[derive(Debug, Default, Clone)]
pub struct RecallSet {
	/// Oldest first, which is the order rustyline indexes by.
	entries: Vec<String>,
	/// Supervisors named in the log, newest first, for the write-mode prompt.
	supervisors: Vec<String>,
}

impl RecallSet {
	/// Build the recall set by reading the log newest first.
	///
	/// Reading stops as soon as the budget is met, and files are opened newest
	/// first, so how many segments the directory holds barely affects startup.
	pub fn build(dir: &Path) -> Self {
		let mut set = Self::default();
		let mut budget = RECALL_BUDGET;
		let mut seen_supervisors = std::collections::HashSet::new();
		let mut collected: Vec<(jiff::Timestamp, u64, String)> = Vec::new();

		let Ok(files) = paths::list(dir) else {
			return set;
		};

		for (path, kind) in files.into_iter().rev() {
			if budget == 0 {
				break;
			}

			for record in newest_first(&path, kind, budget) {
				match &record.kind {
					RecordKind::Context(ContextRecord { ots: Some(ots), .. })
						if !ots.is_empty() && seen_supervisors.insert(ots.clone()) =>
					{
						set.supervisors.push(ots.clone());
					}
					RecordKind::Query(query) if query.source.is_recallable() => {
						if query.query.len() > RECALL_CUTOFF {
							continue;
						}
						if query.query.len() > budget {
							budget = 0;
							break;
						}
						budget -= query.query.len();
						collected.push((record.ts, record.seq, query.query.clone()));
					}
					_ => {}
				}
			}
		}

		// Files are read newest first, which is what keeps startup flat, but
		// concurrent sessions on one day live in separate files. Ordering what
		// was collected by time puts them back into the order they were run,
		// and the budget bounds how much there is to order.
		collected.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
		// An interrupted compaction leaves a record in both a day file and the
		// segment it was folded from. The copies are byte-identical, so an
		// operator should not see the same statement twice for it.
		collected.dedup();
		set.entries = collected.into_iter().map(|(_, _, query)| query).collect();
		debug!(
			entries = set.entries.len(),
			supervisors = set.supervisors.len(),
			"built recall set"
		);
		set
	}

	/// Add a statement the session just ran, so what an operator can recall in
	/// the session that ran them is what a later session would recall too.
	pub fn push(&mut self, query: String) {
		self.entries.push(query);
	}

	/// Supervisors named in the log, newest first.
	pub fn supervisors(&self) -> &[String] {
		&self.supervisors
	}

	pub fn len(&self) -> usize {
		self.entries.len()
	}

	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}

	pub fn get(&self, index: usize) -> Option<&str> {
		self.entries.get(index).map(String::as_str)
	}

	fn hits(
		&self,
		start: usize,
		dir: SearchDirection,
		matches: impl Fn(&str) -> Option<usize>,
	) -> Option<SearchResult<'_>> {
		if start >= self.entries.len() {
			return None;
		}

		let range: Box<dyn Iterator<Item = usize>> = match dir {
			SearchDirection::Forward => Box::new(start..self.entries.len()),
			SearchDirection::Reverse => Box::new((0..=start).rev()),
		};

		for index in range {
			let entry = &self.entries[index];
			if let Some(pos) = matches(entry) {
				return Some(SearchResult {
					entry: Cow::Borrowed(entry),
					idx: index,
					pos,
				});
			}
		}

		None
	}
}

/// A file's records, newest first, holding no more than the budget needs.
///
/// A plain segment is walked backwards from its end, so a reader that has met
/// its budget stops without the rest of the file ever being touched. A day file
/// has to be decompressed forward, so its records cannot be reached from the end
/// — but only the newest of them can matter, so the pass keeps a tail bounded by
/// the same budget and lets everything ahead of it go.
fn newest_first(path: &Path, kind: AuditFile, budget: usize) -> Box<dyn Iterator<Item = Record>> {
	match kind {
		AuditFile::Segment { .. } => match ReverseFramedReader::open(path) {
			Ok(reader) => Box::new(reader.filter_map(FramedItem::into_record)),
			Err(err) => {
				debug!(?err, ?path, "skipping unreadable audit segment");
				Box::new(std::iter::empty())
			}
		},
		AuditFile::DayFile { .. } => match open(path, kind) {
			Ok(reader) => Box::new(bounded_tail(reader, budget).into_iter().rev()),
			Err(err) => {
				debug!(?err, ?path, "skipping unreadable audit day file");
				Box::new(std::iter::empty())
			}
		},
	}
}

/// The last records of a forward pass, bounded by how much text the caller can
/// still take.
///
/// Returned in file order, so the caller reverses to get newest first. Context
/// records are kept whatever else goes, since a supervisor named anywhere in the
/// file is wanted and a name costs nothing against a budget measured in query
/// text; they are put where reversing brings them out first, ahead of any point
/// the caller might stop at.
fn bounded_tail(reader: impl Iterator<Item = FramedItem>, budget: usize) -> Vec<Record> {
	let mut contexts = Vec::new();
	let mut tail: VecDeque<Record> = VecDeque::new();
	let mut held = 0usize;

	for record in reader.filter_map(FramedItem::into_record) {
		match &record.kind {
			RecordKind::Context(_) => contexts.push(record),
			RecordKind::Query(query)
				if query.source.is_recallable() && query.query.len() <= RECALL_CUTOFF =>
			{
				held += query.query.len();
				tail.push_back(record);

				while held > budget && tail.len() > 1 {
					let Some(dropped) = tail.pop_front() else {
						break;
					};
					if let RecordKind::Query(query) = &dropped.kind {
						held -= query.query.len();
					}
				}
			}
			_ => {}
		}
	}

	let mut kept: Vec<Record> = tail.into();
	kept.append(&mut contexts);
	kept
}

impl History for super::Audit {
	fn get(
		&self,
		index: usize,
		_dir: SearchDirection,
	) -> rustyline::Result<Option<SearchResult<'_>>> {
		Ok(self.recall.get(index).map(|entry| SearchResult {
			entry: Cow::Borrowed(entry),
			idx: index,
			pos: 0,
		}))
	}

	fn add(&mut self, _line: &str) -> rustyline::Result<bool> {
		// The session adds to its own recall set as it records, so that what is
		// recalled and what is logged cannot drift apart.
		Ok(true)
	}

	fn add_owned(&mut self, _line: String) -> rustyline::Result<bool> {
		Ok(true)
	}

	fn len(&self) -> usize {
		self.recall.len()
	}

	fn is_empty(&self) -> bool {
		self.recall.is_empty()
	}

	fn set_max_len(&mut self, _len: usize) -> rustyline::Result<()> {
		// The recall set is bounded by its own memory budget.
		Ok(())
	}

	fn ignore_dups(&mut self, _yes: bool) -> rustyline::Result<()> {
		// Every statement an operator ran is recalled, repeats included.
		Ok(())
	}

	fn ignore_space(&mut self, _yes: bool) {}

	fn save(&mut self, _path: &Path) -> rustyline::Result<()> {
		// History is the audit log, which is already written as it goes.
		Ok(())
	}

	fn append(&mut self, _path: &Path) -> rustyline::Result<()> {
		Ok(())
	}

	fn load(&mut self, _path: &Path) -> rustyline::Result<()> {
		Ok(())
	}

	fn clear(&mut self) -> rustyline::Result<()> {
		// An audit log is not something a session can clear.
		Ok(())
	}

	fn search(
		&self,
		term: &str,
		start: usize,
		dir: SearchDirection,
	) -> rustyline::Result<Option<SearchResult<'_>>> {
		Ok(self.recall.hits(start, dir, |entry| entry.find(term)))
	}

	fn starts_with(
		&self,
		term: &str,
		start: usize,
		dir: SearchDirection,
	) -> rustyline::Result<Option<SearchResult<'_>>> {
		Ok(self
			.recall
			.hits(start, dir, |entry| entry.starts_with(term).then_some(0)))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::audit::{
		compact,
		record::QuerySource,
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

	fn entries(dir: &Path) -> Vec<String> {
		let set = RecallSet::build(dir);
		(0..set.len())
			.map(|i| set.get(i).unwrap().to_string())
			.collect()
	}

	#[test]
	fn recall_is_oldest_first_across_the_whole_log() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		for i in 0..5 {
			writer.query(&context(), format!("select {i};"), QuerySource::Typed);
		}
		drop(writer);

		assert_eq!(
			entries(dir.path()),
			vec![
				"select 0;",
				"select 1;",
				"select 2;",
				"select 3;",
				"select 4;"
			]
		);
	}

	#[test]
	fn only_statements_typed_at_the_prompt_are_recalled() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "\\i fixups.sql".into(), QuerySource::Typed);
		writer.query(
			&context(),
			"update patients;".into(),
			QuerySource::Include {
				path: "/tmp/fixups.sql".into(),
			},
		);
		writer.query(
			&context(),
			"select from snippet;".into(),
			QuerySource::Snippet {
				name: "counts".into(),
			},
		);
		writer.query(&context(), "select 1;".into(), QuerySource::Typed);
		drop(writer);

		assert_eq!(
			entries(dir.path()),
			vec!["\\i fixups.sql", "select 1;"],
			"the invoking line is recalled, what it expanded to is not"
		);
	}

	#[test]
	fn a_statement_over_the_cutoff_is_left_out_but_stays_in_the_log() {
		let dir = tempfile::tempdir().unwrap();
		let huge = format!("select '{}';", "x".repeat(RECALL_CUTOFF));
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "before;".into(), QuerySource::Typed);
		writer.query(&context(), huge.clone(), QuerySource::Typed);
		writer.query(&context(), "after;".into(), QuerySource::Typed);
		drop(writer);

		assert_eq!(entries(dir.path()), vec!["before;", "after;"]);

		let logged: Vec<_> = crate::audit::read::Reader::open(dir.path())
			.unwrap()
			.entries()
			.map(|e| e.query)
			.collect();
		assert!(logged.contains(&huge), "it is still recorded");
	}

	#[test]
	fn the_budget_bounds_what_is_recalled() {
		let dir = tempfile::tempdir().unwrap();
		// Statements comfortably under the per-record cutoff, and comfortably
		// more of them in total than the budget allows.
		let body = "x".repeat(RECALL_CUTOFF / 2);
		let count = 3 * RECALL_BUDGET / body.len();

		let mut writer = Writer::new(dir.path());
		for i in 0..count {
			writer.query(&context(), format!("{i:08}{body}"), QuerySource::Typed);
		}
		drop(writer);

		let set = RecallSet::build(dir.path());
		let held: usize = (0..set.len()).map(|i| set.get(i).unwrap().len()).sum();
		assert!(held <= RECALL_BUDGET, "held {held} bytes");
		assert!(set.len() < count, "the budget cut something");
		assert!(!set.is_empty());

		// What survives the budget is the newest, which is what an operator
		// reaches for first.
		assert!(
			set.get(set.len() - 1)
				.unwrap()
				.starts_with(&format!("{:08}", count - 1))
		);
	}

	#[test]
	fn recall_spans_sessions_and_day_files() {
		let dir = tempfile::tempdir().unwrap();
		let mut one = Writer::new(dir.path());
		one.query(&context(), "from one;".into(), QuerySource::Typed);
		drop(one);
		let mut two = Writer::new(dir.path());
		two.query(&context(), "from two;".into(), QuerySource::Typed);
		drop(two);

		let before = entries(dir.path());
		assert_eq!(before.len(), 2);

		// Age the segments past the window and fold them, then recall again.
		let old = jiff::Timestamp::now()
			.to_zoned(jiff::tz::TimeZone::UTC)
			.date()
			.checked_sub(jiff::Span::new().days(compact::PLAIN_TEXT_WINDOW_DAYS + 1))
			.unwrap();
		for (path, kind) in paths::list(dir.path()).unwrap() {
			let renamed = dir
				.path()
				.join(paths::segment_name(old, kind.instance().unwrap()));
			std::fs::rename(path, renamed).unwrap();
		}
		compact::run(dir.path()).unwrap();

		assert_eq!(entries(dir.path()), before, "a day file recalls the same");
	}

	#[test]
	fn duplicates_left_by_an_interrupted_fold_recall_once() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "select 1;".into(), QuerySource::Typed);
		writer.query(&context(), "select 2;".into(), QuerySource::Typed);
		drop(writer);

		let old = jiff::Timestamp::now()
			.to_zoned(jiff::tz::TimeZone::UTC)
			.date()
			.checked_sub(jiff::Span::new().days(compact::PLAIN_TEXT_WINDOW_DAYS + 1))
			.unwrap();
		let (path, kind) = paths::list(dir.path()).unwrap().pop().unwrap();
		let aged = dir
			.path()
			.join(paths::segment_name(old, kind.instance().unwrap()));
		std::fs::rename(&path, &aged).unwrap();

		let kept = std::fs::read(&aged).unwrap();
		compact::run(dir.path()).unwrap();
		// Put the segment back, as an interruption between the rename and the
		// delete would have.
		std::fs::write(&aged, kept).unwrap();

		assert_eq!(entries(dir.path()), vec!["select 1;", "select 2;"]);
	}

	#[test]
	fn supervisors_are_collected_newest_first() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		for name in ["Alice", "Bob", "Alice"] {
			writer.query(
				&Context {
					writemode: true,
					ots: Some(name.into()),
					..context()
				},
				format!("update as {name};"),
				QuerySource::Typed,
			);
		}
		drop(writer);

		assert_eq!(
			RecallSet::build(dir.path()).supervisors(),
			&["Alice".to_string(), "Bob".to_string()]
		);
	}

	#[test]
	fn a_day_file_far_larger_than_the_budget_still_recalls_the_newest() {
		let dir = tempfile::tempdir().unwrap();
		let body = "x".repeat(RECALL_CUTOFF / 2);
		let count = 4 * RECALL_BUDGET / body.len();

		let mut writer = Writer::new(dir.path());
		for i in 0..count {
			writer.query(&context(), format!("{i:08}{body}"), QuerySource::Typed);
		}
		drop(writer);

		// Fold it, so the whole lot has to be read forward out of one file.
		let old = jiff::Timestamp::now()
			.to_zoned(jiff::tz::TimeZone::UTC)
			.date()
			.checked_sub(jiff::Span::new().days(compact::PLAIN_TEXT_WINDOW_DAYS + 1))
			.unwrap();
		let (path, kind) = paths::list(dir.path()).unwrap().pop().unwrap();
		std::fs::rename(
			path,
			dir.path()
				.join(paths::segment_name(old, kind.instance().unwrap())),
		)
		.unwrap();
		compact::run(dir.path()).unwrap();

		let set = RecallSet::build(dir.path());
		let held: usize = (0..set.len()).map(|i| set.get(i).unwrap().len()).sum();
		assert!(held <= RECALL_BUDGET, "held {held} bytes");
		assert!(
			set.get(set.len() - 1)
				.unwrap()
				.starts_with(&format!("{:08}", count - 1)),
			"the newest statement is still what an operator reaches first"
		);
	}

	#[test]
	fn an_empty_log_recalls_nothing() {
		let dir = tempfile::tempdir().unwrap();
		assert!(RecallSet::build(dir.path()).is_empty());
	}
}
