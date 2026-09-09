//! Importing a store left in the earlier single-file database format.
//!
//! A directory holding one, including any working-copy or orphaned files that
//! format left behind, is imported the first time any process opens it, session
//! or tool alike, so an auditor reading a machine that has not run a session
//! since sees what a session would.
//!
//! spec: AUD-STO

use std::{
	collections::{HashMap, HashSet},
	io::{BufWriter, Write as _},
	path::{Path, PathBuf},
};

use jiff::Timestamp;
use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use redb::{Database, ReadableDatabase as _, ReadableTable as _, TableDefinition};
use serde::Deserialize;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{
	lock::Lock,
	paths,
	record::{
		ContextRecord, FORMAT_VERSION, QueryRecord, QuerySource, Record, RecordKind, frame, hash,
	},
	tailscale::TailscalePeer,
	writer::date_of,
};

/// The table the old store kept its records in, keyed by microsecond timestamp.
const HISTORY_TABLE: TableDefinition<'_, u64, &str> = TableDefinition::new("history");

/// A record as the old store wrote it.
#[derive(Debug, Deserialize)]
struct LegacyEntry {
	query: String,
	#[serde(default)]
	db_user: String,
	#[serde(default)]
	sys_user: String,
	#[serde(default)]
	writemode: bool,
	#[serde(default)]
	tailscale: Vec<TailscalePeer>,
	#[serde(default)]
	ots: Option<String>,
	/// Which instance recorded the record, where the old store knew.
	#[serde(default)]
	instance_id: Option<Uuid>,
	/// Whether the old store held the record eligible for recall.
	#[serde(default = "yes")]
	recall: bool,
}

fn yes() -> bool {
	true
}

/// The identity standing for records the old store did not attribute.
///
/// The same set of files always gives the same identity, so an interrupted
/// import writes the same segment names when it is tried again.
fn anonymous_identity(files: &[std::path::PathBuf]) -> Uuid {
	use sha2::{Digest as _, Sha256};

	let mut hasher = Sha256::new();
	for path in files {
		hasher.update(path.as_os_str().as_encoded_bytes());
		hasher.update([0]);
	}

	let digest = hasher.finalize();
	let mut bytes = [0u8; 16];
	bytes.copy_from_slice(&digest[..16]);
	Uuid::from_bytes(bytes)
}

/// One imported session's running chain.
///
/// The chain runs for the life of the session rather than the life of a file,
/// so a session the old store recorded over several days keeps one numbering and
/// one chain across the segments it is split into.
struct Session {
	next_seq: u64,
	prev: String,
	/// The context last written, and the day it was written on: a new segment
	/// opens with a context record even when nothing about the context changed.
	context: Option<(jiff::civil::Date, ContextRecord)>,
}

/// The segments being written, kept open across records.
///
/// A full legacy store held hundreds of thousands of records, and reopening the
/// destination for each one would put that many open and close calls on the
/// session's startup path. A bounded set is kept open instead, because one
/// descriptor per session-day would run past the open-file limit on a store
/// spanning months.
///
/// Everything is written under a name no reader looks at, and moved into place
/// only once every legacy file has been read right through. An import that fails
/// partway therefore leaves nothing behind: the alternative is a half-written
/// chain that the next attempt appends a second chain onto, which reads as
/// permanently broken and which nothing folds away.
#[derive(Default)]
struct Open {
	/// Writers open right now, most recently used last.
	writing: Vec<(Key, PathBuf, BufWriter<std::fs::File>)>,
	/// Every segment this import has written to, whether still open or not.
	started: Vec<(Key, PathBuf)>,
}

type Key = (Uuid, jiff::civil::Date);

/// How many segments are written to at once.
///
/// A legacy store spanning months with a few sessions a day covers a thousand
/// or more session-days, and one descriptor each would run past the open-file
/// limit long before the import finished — which, since import is all or
/// nothing, would mean it never finished. Writers beyond this are closed and
/// reopened as the rows call for them; the old store is keyed by timestamp, so
/// rows arrive in time order and a handful of writers covers the days in play.
const OPEN_AT_ONCE: usize = 32;

impl Open {
	fn writer(
		&mut self,
		dir: &Path,
		instance: Uuid,
		date: jiff::civil::Date,
	) -> Result<&mut BufWriter<std::fs::File>> {
		let key = (instance, date);

		if let Some(at) = self.writing.iter().position(|(open, _, _)| *open == key) {
			// Most recently used goes last, so the front is what gets closed.
			let entry = self.writing.remove(at);
			self.writing.push(entry);
			let (_, _, writer) = self.writing.last_mut().expect("just pushed");
			return Ok(writer);
		}

		while self.writing.len() >= OPEN_AT_ONCE {
			let (_, path, mut writer) = self.writing.remove(0);
			writer
				.flush()
				.into_diagnostic()
				.wrap_err_with(|| format!("writing {}", path.display()))?;
		}

		let final_path = dir.join(paths::segment_name(date, instance));
		let path = paths::part_of(&final_path);
		let first = !self.started.iter().any(|(open, _)| *open == key);

		// Created exclusively the first time, so a symlink left at this
		// predictable name cannot have the import write through it, and appended
		// to on reopening. Anything already there is the wreckage of an import
		// that did not finish, and goes first.
		let file = if first {
			if path.exists() {
				std::fs::remove_file(&path)
					.into_diagnostic()
					.wrap_err_with(|| format!("clearing {}", path.display()))?;
			}
			self.started.push((key, final_path));
			paths::private().create_new(true).write(true).open(&path)
		} else {
			paths::private().append(true).open(&path)
		}
		.into_diagnostic()
		.wrap_err_with(|| format!("opening {}", path.display()))?;

		self.writing.push((key, path, BufWriter::new(file)));
		let (_, _, writer) = self.writing.last_mut().expect("just pushed");
		Ok(writer)
	}

	/// Get everything buffered onto disk and move it into place.
	fn adopt(mut self) -> Result<()> {
		for (_, path, mut writer) in std::mem::take(&mut self.writing) {
			writer
				.flush()
				.into_diagnostic()
				.wrap_err_with(|| format!("writing {}", path.display()))?;
			writer
				.into_inner()
				.map_err(|err| miette!("flushing an imported audit segment: {err}"))?
				.sync_all()
				.into_diagnostic()?;
		}

		for (_, final_path) in &self.started {
			let part = paths::part_of(final_path);
			// Reopened segments were synced when they were last closed; syncing
			// the file again here covers the ones that were.
			if let Ok(file) = std::fs::File::open(&part) {
				file.sync_all().ok();
			}
			std::fs::rename(&part, final_path)
				.into_diagnostic()
				.wrap_err_with(|| format!("moving {} into place", part.display()))?;
		}

		Ok(())
	}

	/// Throw away what was written, so a later attempt starts clean.
	fn abandon(mut self) {
		self.writing.clear();
		for (_, final_path) in &self.started {
			let part = paths::part_of(final_path);
			if let Err(err) = std::fs::remove_file(&part) {
				debug!(?err, ?part, "could not clear an unfinished import");
			}
		}
	}
}

/// Import any legacy store in `dir`.
///
/// Runs under the directory lock, which the caller already holds; a process that
/// could not take it reads what is already there and leaves the import to
/// whoever holds it.
pub fn import(dir: &Path, _lock: &Lock) -> Result<usize> {
	let files = paths::list_legacy(dir)?;
	if files.is_empty() {
		return Ok(0);
	}

	info!(count = files.len(), "importing legacy audit store");

	// One identity stands for every record the old store did not attribute, so
	// that every segment in the directory is named the same way.
	//
	// Derived from the names being imported rather than made fresh, so that an
	// attempt interrupted after some segments were moved into place writes the
	// same names again and settles over them. A fresh identity each time would
	// leave the ones already moved beside a second set under another name, and
	// the same statements would be in the log twice.
	let anonymous = anonymous_identity(&files);

	// The old store kept one table keyed by microsecond timestamp, and a working
	// copy was made by copying the whole database, so the same record is in main
	// and in every copy taken after it. The key is that record's identity: what
	// has already been taken across is not taken again.
	let mut taken: HashSet<u64> = HashSet::new();
	let mut sessions: HashMap<Uuid, Session> = HashMap::new();
	let mut open = Open::default();
	let mut imported = 0;

	// All of it or none of it. A legacy file that cannot be read right through
	// leaves the whole import to be attempted again from the start, because the
	// records it did yield are interleaved with every other file's in the same
	// segments and cannot be adopted on their own.
	for path in &files {
		match import_file(dir, path, anonymous, &mut sessions, &mut open, &mut taken) {
			Ok(count) => imported += count,
			Err(err) => {
				warn!(?err, ?path, "could not import a legacy audit file");
				open.abandon();
				return Ok(0);
			}
		}
	}

	// The old files are set aside only after the new segments have been written
	// and synchronised to disk.
	//
	// Set aside, not deleted: import runs from the read-only tools too, so an
	// auditor who points `verify` at a machine's store would otherwise destroy
	// the very files they came to examine. The name keeps them from being
	// imported a second time, and leaves them there to be compared against
	// until retention takes them like anything else.
	open.adopt()?;
	let today = super::writer::date_of(Timestamp::now());
	for path in &files {
		let aside = paths::set_aside(path, today);
		if let Err(err) = std::fs::rename(path, &aside) {
			warn!(
				?err,
				?path,
				"could not set an imported legacy audit file aside"
			);
		}
	}

	info!(records = imported, "imported legacy audit store");
	Ok(imported)
}

/// Stream one legacy file's records into segments.
///
/// Records are read one at a time rather than the store being loaded whole. One
/// key per record taken across is held, though, because the same record is in
/// the main file and in every copy taken after it and a statement that ran once
/// belongs in the log once; that is bounded by the old store's own size limit
/// rather than being flat. Merging the files' cursors would compare against
/// only the last key emitted and make it so.
fn import_file(
	dir: &Path,
	path: &Path,
	anonymous: Uuid,
	sessions: &mut HashMap<Uuid, Session>,
	open: &mut Open,
	taken: &mut HashSet<u64>,
) -> Result<usize> {
	let db = Database::open(path)
		.into_diagnostic()
		.wrap_err_with(|| format!("opening legacy audit store {}", path.display()))?;
	let read = db.begin_read().into_diagnostic()?;
	let Ok(table) = read.open_table(HISTORY_TABLE) else {
		debug!(?path, "legacy audit file holds no records");
		return Ok(0);
	};

	let mut imported = 0;
	for row in table.iter().into_diagnostic()? {
		let (key, value) = row.into_diagnostic()?;
		if !taken.insert(key.value()) {
			// Already taken across from an earlier file: main and its copies
			// hold the same records, and a statement ran once.
			continue;
		}

		let Ok(entry) = serde_json::from_str::<LegacyEntry>(value.value()) else {
			warn!(
				?path,
				key = key.value(),
				"skipping unreadable legacy record"
			);
			continue;
		};

		// Imported records keep their original timestamps. The key is a count of
		// microseconds that the old store never bounded, so one out of range is
		// skipped rather than wrapped around into some other moment.
		let ts = i64::try_from(key.value())
			.ok()
			.and_then(|micros| Timestamp::from_microsecond(micros).ok());
		let Some(ts) = ts else {
			warn!(
				?path,
				key = key.value(),
				"skipping legacy record with an impossible timestamp"
			);
			continue;
		};

		let instance = entry.instance_id.unwrap_or(anonymous);
		write_entry(dir, sessions, open, instance, ts, entry)?;
		imported += 1;
	}

	Ok(imported)
}

fn write_entry(
	dir: &Path,
	sessions: &mut HashMap<Uuid, Session>,
	open: &mut Open,
	instance: Uuid,
	ts: Timestamp,
	entry: LegacyEntry,
) -> Result<()> {
	let date = date_of(ts);
	let session = sessions.entry(instance).or_insert_with(|| Session {
		next_seq: 0,
		prev: String::new(),
		context: None,
	});

	let context = ContextRecord {
		sys_user: entry.sys_user,
		db_user: entry.db_user,
		writemode: entry.writemode,
		ots: entry.ots,
		tailscale: entry.tailscale,
		instance,
	};

	let file = open.writer(dir, instance, date)?;

	// The first record of every segment is a context record, and a new one goes
	// in whenever the state it carries changes.
	if session.context.as_ref() != Some(&(date, context.clone())) {
		session.context = Some((date, context.clone()));
		append(file, session, ts, RecordKind::Context(context))?;
	}

	append(
		file,
		session,
		ts,
		RecordKind::Query(QueryRecord {
			query: entry.query,
			// The old store recorded that a statement was not typed without
			// recording what ran it, so that is all the source can say.
			source: if entry.recall {
				QuerySource::Typed
			} else {
				QuerySource::Unknown
			},
		}),
	)?;

	Ok(())
}

fn append(
	file: &mut BufWriter<std::fs::File>,
	session: &mut Session,
	ts: Timestamp,
	kind: RecordKind,
) -> Result<()> {
	let record = Record {
		v: FORMAT_VERSION,
		seq: session.next_seq,
		ts,
		prev: session.prev.clone(),
		kind,
	};
	session.next_seq += 1;

	let json = record.to_json().into_diagnostic()?;
	file.write_all(&frame(&json)).into_diagnostic()?;
	session.prev = hash(&json);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::audit::{read::Reader, verify::verify};
	use redb::ReadableTableMetadata as _;

	fn legacy_store(path: &Path, entries: &[(u64, serde_json::Value)]) {
		let db = Database::create(path).unwrap();
		let write = db.begin_write().unwrap();
		{
			let mut table = write.open_table(HISTORY_TABLE).unwrap();
			for (ts, entry) in entries {
				table.insert(*ts, entry.to_string().as_str()).unwrap();
			}
			assert!(table.len().unwrap() > 0);
		}
		write.commit().unwrap();
	}

	fn entry(query: &str, instance: Option<&str>, recall: bool) -> serde_json::Value {
		serde_json::json!({
			"query": query,
			"db_user": "tamanu",
			"sys_user": "felix",
			"writemode": false,
			"tailscale": [],
			"ots": null,
			"instance_id": instance,
			"recall": recall,
		})
	}

	/// Microseconds since the epoch for a given day, plus an offset.
	fn at(day: &str, offset: u64) -> u64 {
		let ts: Timestamp = format!("{day}T00:00:00Z").parse().unwrap();
		ts.as_microsecond() as u64 + offset
	}

	#[test]
	fn a_legacy_store_is_imported_into_segments() {
		let dir = tempfile::tempdir().unwrap();
		legacy_store(
			&dir.path().join("audit-main.redb"),
			&[
				(at("2026-01-01", 1), entry("select 1;", None, true)),
				(at("2026-01-01", 2), entry("select 2;", None, true)),
			],
		);

		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		assert_eq!(import(dir.path(), &lock).unwrap(), 2);
		drop(lock);

		assert!(paths::list_legacy(dir.path()).unwrap().is_empty());
		let queries: Vec<_> = Reader::open(dir.path())
			.unwrap()
			.entries()
			.map(|e| e.query)
			.collect();
		assert_eq!(queries, vec!["select 1;", "select 2;"]);
	}

	#[test]
	fn imported_records_keep_their_timestamps_and_context() {
		let dir = tempfile::tempdir().unwrap();
		legacy_store(
			&dir.path().join("audit-main.redb"),
			&[(at("2026-01-01", 500), entry("select 1;", None, true))],
		);

		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		import(dir.path(), &lock).unwrap();
		drop(lock);

		let entry = Reader::open(dir.path()).unwrap().entries().next().unwrap();
		assert_eq!(entry.ts.as_microsecond() as u64, at("2026-01-01", 500));
		assert_eq!(entry.sys_user, "felix");
		assert_eq!(entry.db_user, "tamanu");
	}

	#[test]
	fn the_old_recall_flag_becomes_a_source() {
		let dir = tempfile::tempdir().unwrap();
		legacy_store(
			&dir.path().join("audit-main.redb"),
			&[
				(at("2026-01-01", 1), entry("typed;", None, true)),
				(at("2026-01-01", 2), entry("not typed;", None, false)),
			],
		);

		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		import(dir.path(), &lock).unwrap();
		drop(lock);

		let sources: Vec<_> = Reader::open(dir.path())
			.unwrap()
			.entries()
			.map(|e| (e.query, e.source))
			.collect();
		assert_eq!(
			sources,
			vec![
				("typed;".to_string(), QuerySource::Typed),
				("not typed;".to_string(), QuerySource::Unknown),
			]
		);
	}

	#[test]
	fn records_are_grouped_by_session_and_day() {
		let dir = tempfile::tempdir().unwrap();
		let one = "11111111-1111-4111-8111-111111111111";
		let two = "22222222-2222-4222-8222-222222222222";
		legacy_store(
			&dir.path().join("audit-main.redb"),
			&[
				(at("2026-01-01", 1), entry("one day one;", Some(one), true)),
				(at("2026-01-01", 2), entry("two day one;", Some(two), true)),
				(at("2026-01-02", 1), entry("one day two;", Some(one), true)),
			],
		);

		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		import(dir.path(), &lock).unwrap();
		drop(lock);

		let found = paths::list(dir.path()).unwrap();
		assert_eq!(found.len(), 3, "one segment per session per day");
		assert!(verify(dir.path()).unwrap().holds());
	}

	#[test]
	fn unattributed_records_go_to_a_segment_per_day_under_one_identity() {
		let dir = tempfile::tempdir().unwrap();
		legacy_store(
			&dir.path().join("audit-main.redb"),
			&[
				(at("2026-01-01", 1), entry("day one;", None, true)),
				(at("2026-01-02", 1), entry("day two;", None, true)),
			],
		);

		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		import(dir.path(), &lock).unwrap();
		drop(lock);

		let found = paths::list(dir.path()).unwrap();
		assert_eq!(found.len(), 2);
		let instances: Vec<_> = found.iter().filter_map(|(_, k)| k.instance()).collect();
		assert_eq!(
			instances[0], instances[1],
			"one identity made for the import"
		);
	}

	#[test]
	fn working_and_orphaned_copies_are_imported_too() {
		let dir = tempfile::tempdir().unwrap();
		legacy_store(
			&dir.path().join("audit-main.redb"),
			&[(at("2026-01-01", 1), entry("from main;", None, true))],
		);
		legacy_store(
			&dir.path().join("audit-working-abc.redb"),
			&[(at("2026-01-01", 2), entry("from working;", None, true))],
		);
		legacy_store(
			&dir.path().join("audit-orphaned-def.redb"),
			&[(at("2026-01-01", 3), entry("from orphaned;", None, true))],
		);

		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		assert_eq!(import(dir.path(), &lock).unwrap(), 3);
		drop(lock);

		let mut queries: Vec<_> = Reader::open(dir.path())
			.unwrap()
			.entries()
			.map(|e| e.query)
			.collect();
		queries.sort();
		assert_eq!(
			queries,
			vec!["from main;", "from orphaned;", "from working;"]
		);
		assert!(paths::list_legacy(dir.path()).unwrap().is_empty());
	}

	#[test]
	fn imported_segments_verify() {
		let dir = tempfile::tempdir().unwrap();
		let entries: Vec<_> = (0..50)
			.map(|i| {
				(
					at("2026-01-01", i),
					entry(&format!("select {i};"), None, true),
				)
			})
			.collect();
		legacy_store(&dir.path().join("audit-main.redb"), &entries);

		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		import(dir.path(), &lock).unwrap();
		drop(lock);

		let report = verify(dir.path()).unwrap();
		assert!(report.holds(), "{:?}", report.sessions);
	}

	#[test]
	fn an_imported_legacy_file_is_set_aside_rather_than_deleted() {
		let dir = tempfile::tempdir().unwrap();
		let original = dir.path().join("audit-main.redb");
		legacy_store(
			&original,
			&[(at("2026-01-01", 1), entry("select 1;", None, true))],
		);

		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		import(dir.path(), &lock).unwrap();
		drop(lock);

		// A read-only tool imports too, so the originals have to survive being
		// looked at: an auditor cannot be the one who destroys the evidence.
		assert!(!original.exists());
		let today = date_of(Timestamp::now());
		assert!(paths::set_aside(&original, today).exists());

		// And they are named for the day they were set aside on, so retention
		// takes them in their turn rather than keeping them for ever.
		assert_eq!(
			paths::list_set_aside(dir.path()).unwrap(),
			vec![(paths::set_aside(&original, today), today)]
		);

		// And they are not imported a second time.
		assert!(paths::list_legacy(dir.path()).unwrap().is_empty());
		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		assert_eq!(import(dir.path(), &lock).unwrap(), 0);
	}

	#[test]
	fn a_record_held_in_more_than_one_legacy_file_is_imported_once() {
		let dir = tempfile::tempdir().unwrap();

		// The old format made a working copy by copying the whole database, so
		// the same records sat in main and in every copy taken after it. A
		// statement ran once and the log must say so once.
		let shared = [
			(at("2026-01-01", 1), entry("select 1;", None, true)),
			(at("2026-01-01", 2), entry("select 2;", None, true)),
		];
		legacy_store(&dir.path().join("audit-main.redb"), &shared);
		let mut working = shared.to_vec();
		working.push((at("2026-01-01", 3), entry("select 3;", None, true)));
		legacy_store(&dir.path().join("audit-working-abc.redb"), &working);

		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		assert_eq!(import(dir.path(), &lock).unwrap(), 3);
		drop(lock);

		let queries: Vec<_> = crate::audit::read::Reader::open(dir.path())
			.unwrap()
			.entries()
			.map(|entry| entry.query)
			.collect();
		assert_eq!(queries, vec!["select 1;", "select 2;", "select 3;"]);
		assert!(crate::audit::verify::verify(dir.path()).unwrap().holds());
	}

	#[test]
	fn the_identity_for_unattributed_records_is_the_same_on_a_retry() {
		let one = anonymous_identity(&[
			std::path::PathBuf::from("/store/audit-main.redb"),
			std::path::PathBuf::from("/store/audit-working-abc.redb"),
		]);
		let again = anonymous_identity(&[
			std::path::PathBuf::from("/store/audit-main.redb"),
			std::path::PathBuf::from("/store/audit-working-abc.redb"),
		]);
		assert_eq!(one, again, "a retry writes the same segment names");

		let different = anonymous_identity(&[std::path::PathBuf::from("/store/audit-main.redb")]);
		assert_ne!(one, different);
	}

	#[test]
	fn a_directory_with_no_legacy_store_imports_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		assert_eq!(import(dir.path(), &lock).unwrap(), 0);
	}
}
