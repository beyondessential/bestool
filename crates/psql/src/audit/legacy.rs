//! Importing a store left in the earlier single-file database format.
//!
//! A directory holding one, including any working-copy or orphaned files that
//! format left behind, is imported the first time any process opens it, session
//! or tool alike, so an auditor reading a machine that has not run a session
//! since sees what a session would.
//!
//! spec: AUD-STO

use std::{collections::HashMap, fs::OpenOptions, io::Write as _, path::Path};

use jiff::Timestamp;
use miette::{IntoDiagnostic as _, Result, WrapErr as _};
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
	let anonymous = Uuid::new_v4();
	let mut sessions: HashMap<Uuid, Session> = HashMap::new();
	let mut imported = 0;

	for path in &files {
		match import_file(dir, path, anonymous, &mut sessions) {
			Ok(count) => imported += count,
			// A legacy file that cannot be read must not stop the others, nor
			// leave the directory permanently stuck trying.
			Err(err) => warn!(?err, ?path, "could not import a legacy audit file"),
		}
	}

	// The old files go only after the new segments have been written and
	// synchronised to disk.
	sync_dir(dir)?;
	for path in &files {
		if let Err(err) = std::fs::remove_file(path) {
			warn!(
				?err,
				?path,
				"could not delete an imported legacy audit file"
			);
		}
	}

	info!(records = imported, "imported legacy audit store");
	Ok(imported)
}

/// Stream one legacy file's records into segments.
///
/// Records are read one at a time rather than loaded whole, so import completes
/// within flat memory regardless of how large the old store grew.
fn import_file(
	dir: &Path,
	path: &Path,
	anonymous: Uuid,
	sessions: &mut HashMap<Uuid, Session>,
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
		let Ok(entry) = serde_json::from_str::<LegacyEntry>(value.value()) else {
			warn!(
				?path,
				key = key.value(),
				"skipping unreadable legacy record"
			);
			continue;
		};

		// Imported records keep their original timestamps.
		let Some(ts) = Timestamp::from_microsecond(key.value() as i64).ok() else {
			warn!(
				?path,
				key = key.value(),
				"skipping legacy record with an impossible timestamp"
			);
			continue;
		};

		let instance = entry.instance_id.unwrap_or(anonymous);
		write_entry(dir, sessions, instance, ts, entry)?;
		imported += 1;
	}

	Ok(imported)
}

fn write_entry(
	dir: &Path,
	sessions: &mut HashMap<Uuid, Session>,
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

	let path = dir.join(paths::segment_name(date, instance));
	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(&path)
		.into_diagnostic()
		.wrap_err_with(|| format!("opening {}", path.display()))?;

	// The first record of every segment is a context record, and a new one goes
	// in whenever the state it carries changes.
	if session.context.as_ref() != Some(&(date, context.clone())) {
		session.context = Some((date, context.clone()));
		append(&mut file, session, ts, RecordKind::Context(context))?;
	}

	append(
		&mut file,
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
	file: &mut std::fs::File,
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

fn sync_dir(dir: &Path) -> Result<()> {
	for (path, _) in paths::list(dir)? {
		if let Ok(file) = std::fs::File::open(&path)
			&& let Err(err) = file.sync_all()
		{
			warn!(
				?err,
				?path,
				"could not synchronise an imported audit segment"
			);
		}
	}
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
	fn a_directory_with_no_legacy_store_imports_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let lock = Lock::try_directory(dir.path()).unwrap().unwrap();
		assert_eq!(import(dir.path(), &lock).unwrap(), 0);
	}
}
