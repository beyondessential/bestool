//! The read API the command-line tools and the rest of bestool consume.
//!
//! The API is the stable surface; the command-line tools are thin wrappers over
//! it.
//!
//! spec: AUD-API

use std::{
	collections::{HashMap, HashSet, VecDeque},
	io::Write,
	path::{Path, PathBuf},
};

use jiff::Timestamp;
use miette::{IntoDiagnostic as _, Result};
use tracing::debug;
use uuid::Uuid;

use super::{
	compact::{self, CompactionReport},
	paths,
	read::{Range, Reader, Stored},
	record::{Record, RecordKind, frame},
	verify::{self, VerifyReport},
};

/// How much of a limit is allocated up front. The limit comes from the command
/// line, so it is a request rather than a size; the window grows into it as
/// records actually arrive.
const WINDOW_HINT: usize = 1024;

/// How the log is narrowed for a read.
#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
	/// Most records to return. `None` or zero means all of them.
	pub limit: Option<usize>,
	/// Take the oldest matching records rather than the newest.
	pub from_oldest: bool,
	/// Only records at or after this time.
	pub since: Option<String>,
	/// Only records at or before this time.
	pub until: Option<String>,
}

impl QueryOptions {
	fn range(&self) -> Result<Range> {
		Ok(Range {
			since: self.since.as_deref().map(parse_time).transpose()?,
			until: self.until.as_deref().map(parse_time).transpose()?,
		})
	}

	fn limit(&self) -> Option<usize> {
		self.limit.filter(|limit| *limit > 0)
	}
}

/// Where to read from, and what to read.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
	/// Audit directory, defaulting to the one a session uses.
	pub audit_path: Option<PathBuf>,
	pub query_options: QueryOptions,
}

/// Parse a time given as a full timestamp, a date and time, or a bare date.
fn parse_time(text: &str) -> Result<Timestamp> {
	if let Ok(ts) = text.parse::<Timestamp>() {
		return Ok(ts);
	}

	jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M:%S", text)
		.or_else(|_| jiff::civil::DateTime::strptime("%Y-%m-%d", text))
		.into_diagnostic()?
		.to_zoned(jiff::tz::TimeZone::system())
		.into_diagnostic()
		.map(|zoned| zoned.timestamp())
}

fn resolve(path: Option<PathBuf>) -> Result<PathBuf> {
	match path {
		Some(path) => Ok(path),
		None => super::default_path(),
	}
}

/// Read the records a filter selects, in time order, oldest first.
///
/// A limit is applied at whichever end the caller asked for, holding no more
/// than that many records in memory, so a caller reading a year of records with
/// a small limit never holds the year.
pub fn stored(dir: &Path, options: &QueryOptions) -> Result<Vec<Stored>> {
	let reader = Reader::open_range(dir, options.range()?)?;

	let Some(limit) = options.limit() else {
		return Ok(reader.collect());
	};

	if options.from_oldest {
		return Ok(reader.take(limit).collect());
	}

	let mut window: VecDeque<Stored> = VecDeque::with_capacity(limit.min(WINDOW_HINT));
	for record in reader {
		if window.len() == limit {
			window.pop_front();
		}
		window.push_back(record);
	}
	Ok(window.into())
}

/// Read the query records a filter selects, each carrying the context in force
/// at it.
pub fn entries(dir: &Path, options: &QueryOptions) -> Result<Vec<super::Entry>> {
	let reader = Reader::open_range(dir, options.range()?)?.entries();

	let Some(limit) = options.limit() else {
		return Ok(reader.collect());
	};

	if options.from_oldest {
		return Ok(reader.take(limit).collect());
	}

	let mut window = VecDeque::with_capacity(limit.min(WINDOW_HINT));
	for entry in reader {
		if window.len() == limit {
			window.pop_front();
		}
		window.push_back(entry);
	}
	Ok(window.into())
}

/// Verify every session's chain across a directory.
pub fn verify_directory(dir: &Path) -> Result<VerifyReport> {
	verify::verify(dir)
}

/// The current chain head of every session, which is what an off-box witness
/// would publish.
pub fn chain_heads(dir: &Path) -> Result<Vec<(Uuid, String)>> {
	Ok(verify::verify(dir)?
		.sessions
		.into_iter()
		.map(|session| (session.instance, session.head))
		.collect())
}

/// Run compaction and retention once.
pub fn compact_directory(dir: &Path) -> Result<CompactionReport> {
	compact::run(dir)
}

/// Write records to a stream in the shape and framing they have in the store.
///
/// Like compaction, export changes their container and not their content, so an
/// unfiltered export is the log itself, merged into time order and decompressed,
/// and verifies as such.
///
/// Where a filter narrows the output, the context record in force at the start
/// of it is emitted first for each session that appears, so every query record
/// in the output can still be attributed.
pub fn write_export(out: &mut impl Write, dir: &Path, options: &QueryOptions) -> Result<()> {
	let selected = stored(dir, options)?;

	let range = options.range()?;
	let narrowed = range.since.is_some() || range.until.is_some() || options.limit().is_some();

	if narrowed {
		for record in leading_contexts(dir, &selected)? {
			write_record(out, &record)?;
		}
	}

	for record in &selected {
		write_record(out, &record.record)?;
	}

	out.flush().into_diagnostic()
}

/// The context record in force at the start of the output, for each session
/// whose own context record did not make it into the selection.
fn leading_contexts(dir: &Path, selected: &[Stored]) -> Result<Vec<Record>> {
	// A session needs a leading context record when the output starts before its
	// own first context record does. Whichever kind of record comes first for a
	// session settles it: a context change later in the output does not cover
	// the records ahead of it.
	let mut wanted: HashMap<Uuid, (Timestamp, u64)> = HashMap::new();
	let mut covered: HashSet<Uuid> = HashSet::new();
	for stored in selected {
		let Some(instance) = stored.instance else {
			continue;
		};
		if covered.contains(&instance) || wanted.contains_key(&instance) {
			continue;
		}
		match &stored.record.kind {
			RecordKind::Context(_) => {
				covered.insert(instance);
			}
			_ => {
				wanted.insert(instance, (stored.record.ts, stored.record.seq));
			}
		}
	}

	let Some(stop) = wanted.values().map(|(ts, _)| *ts).max() else {
		return Ok(Vec::new());
	};

	// Read from the beginning of the log up to where the output starts, keeping
	// the latest context record of each session that needs one.
	//
	// Each session's own boundary is its first selected record by sequence
	// number as well as timestamp: a clock coarse enough to give a context
	// record and the query after it the same timestamp would otherwise put the
	// boundary on the wrong side of the very record being looked for.
	let mut latest: HashMap<Uuid, Record> = HashMap::new();
	for stored in Reader::open_range(
		dir,
		Range {
			since: None,
			until: Some(stop),
		},
	)? {
		let Some(instance) = stored.instance else {
			continue;
		};
		let Some(boundary) = wanted.get(&instance) else {
			continue;
		};
		if (stored.record.ts, stored.record.seq) >= *boundary {
			continue;
		}
		if matches!(stored.record.kind, RecordKind::Context(_)) {
			latest.insert(instance, stored.record);
		}
	}

	let mut found: Vec<Record> = latest.into_values().collect();
	found.sort_by_key(|record| (record.ts, record.seq));
	Ok(found)
}

fn write_record(out: &mut impl Write, record: &Record) -> Result<()> {
	let json = record.to_json().into_diagnostic()?;
	out.write_all(&frame(&json)).into_diagnostic()
}

/// Export records to standard output.
pub fn export_audit_entries(options: ExportOptions) -> Result<()> {
	let dir = resolve(options.audit_path)?;
	debug!(?dir, "exporting audit records");

	// A tool reading a machine that has not run a session since still sees what
	// a session would.
	import_if_legacy(&dir)?;

	let mut out = std::io::stdout().lock();
	write_export(&mut out, &dir, &options.query_options)
}

/// Bring a legacy store across, if the directory holds one and nothing else is
/// already doing it.
pub fn import_if_legacy(dir: &Path) -> Result<()> {
	if paths::list_legacy(dir)?.is_empty() {
		return Ok(());
	}
	let Some(lock) = super::lock::Lock::try_directory(dir)? else {
		debug!("another process holds the audit directory, leaving the import to it");
		return Ok(());
	};
	super::legacy::import(dir, &lock)?;
	Ok(())
}

/// Whether an error is a closed output pipe, which ends an export quietly.
pub fn is_broken_pipe(err: &miette::Report) -> bool {
	let text = format!("{err:?}");
	text.contains("Broken pipe") || text.contains("BrokenPipe")
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::audit::{
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

	fn log_of(dir: &Path, count: usize) {
		let mut writer = Writer::new(dir);
		for i in 0..count {
			writer.query(&context(), format!("select {i};"), QuerySource::Typed);
			std::thread::sleep(std::time::Duration::from_millis(1));
		}
	}

	fn queries(entries: &[super::super::Entry]) -> Vec<String> {
		entries.iter().map(|e| e.query.clone()).collect()
	}

	#[test]
	fn an_unfiltered_export_is_the_log_itself() {
		let dir = tempfile::tempdir().unwrap();
		log_of(dir.path(), 5);

		let mut out = Vec::new();
		write_export(&mut out, dir.path(), &QueryOptions::default()).unwrap();

		// It reads back as the same records, and verifies as such.
		let exported = dir.path().join("exported");
		std::fs::create_dir(&exported).unwrap();
		std::fs::write(
			exported.join(paths::segment_name(
				jiff::Timestamp::now()
					.to_zoned(jiff::tz::TimeZone::UTC)
					.date(),
				verify::verify(dir.path()).unwrap().sessions[0].instance,
			)),
			&out,
		)
		.unwrap();

		let original: Vec<_> = Reader::open(dir.path()).unwrap().map(|s| s.json).collect();
		let round_tripped: Vec<_> = Reader::open(&exported).unwrap().map(|s| s.json).collect();
		assert_eq!(original, round_tripped);
		assert!(verify::verify(&exported).unwrap().holds());
	}

	#[test]
	fn a_limit_takes_the_newest_by_default() {
		let dir = tempfile::tempdir().unwrap();
		log_of(dir.path(), 10);

		let found = entries(
			dir.path(),
			&QueryOptions {
				limit: Some(3),
				..Default::default()
			},
		)
		.unwrap();
		assert_eq!(queries(&found), vec!["select 7;", "select 8;", "select 9;"]);
	}

	#[test]
	fn a_limit_can_take_the_oldest_instead() {
		let dir = tempfile::tempdir().unwrap();
		log_of(dir.path(), 10);

		let found = entries(
			dir.path(),
			&QueryOptions {
				limit: Some(3),
				from_oldest: true,
				..Default::default()
			},
		)
		.unwrap();
		assert_eq!(queries(&found), vec!["select 0;", "select 1;", "select 2;"]);
	}

	#[test]
	fn a_zero_limit_means_everything() {
		let dir = tempfile::tempdir().unwrap();
		log_of(dir.path(), 6);

		let found = entries(
			dir.path(),
			&QueryOptions {
				limit: Some(0),
				..Default::default()
			},
		)
		.unwrap();
		assert_eq!(found.len(), 6);
	}

	#[test]
	fn a_narrowed_export_still_carries_the_context() {
		let dir = tempfile::tempdir().unwrap();
		log_of(dir.path(), 10);

		let mut out = Vec::new();
		write_export(
			&mut out,
			dir.path(),
			&QueryOptions {
				limit: Some(2),
				..Default::default()
			},
		)
		.unwrap();

		let text = String::from_utf8(out).unwrap();
		assert!(
			text.contains(r#""kind":"context""#),
			"the context in force is emitted first"
		);
		let context_at = text.find(r#""kind":"context""#).unwrap();
		let query_at = text.find(r#""kind":"query""#).unwrap();
		assert!(context_at < query_at);
	}

	#[test]
	fn a_context_change_inside_the_export_does_not_cover_what_precedes_it() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "read only;".into(), QuerySource::Typed);
		writer.query(
			&Context {
				writemode: true,
				ots: Some("Carol".into()),
				..context()
			},
			"write one;".into(),
			QuerySource::Typed,
		);
		drop(writer);

		// The selection holds a context record, but it sits after the first
		// query, so that query still needs the one in force before the output.
		let mut out = Vec::new();
		write_export(
			&mut out,
			dir.path(),
			&QueryOptions {
				limit: Some(4),
				..Default::default()
			},
		)
		.unwrap();

		let exported = dir.path().join("exported");
		std::fs::create_dir(&exported).unwrap();
		std::fs::write(
			exported.join(paths::segment_name(
				jiff::Timestamp::now()
					.to_zoned(jiff::tz::TimeZone::UTC)
					.date(),
				verify::verify(dir.path()).unwrap().sessions[0].instance,
			)),
			&out,
		)
		.unwrap();

		for entry in Reader::open(&exported).unwrap().entries() {
			assert_eq!(
				entry.sys_user, "felix",
				"{} lost its user in the export",
				entry.query
			);
		}
	}

	#[test]
	fn a_leading_context_is_found_even_when_it_shares_a_timestamp() {
		use crate::audit::record::{ContextRecord, FORMAT_VERSION, QueryRecord, RecordKind, frame};

		// A clock coarse enough to give a context record and the query after it
		// the same timestamp is what macOS hands out; the boundary between what
		// is in the output and what precedes it has to hold there too.
		let dir = tempfile::tempdir().unwrap();
		let instance = uuid::Uuid::new_v4();
		let ts: Timestamp = "2026-09-08T03:14:15Z".parse().unwrap();

		let mut records = vec![Record {
			v: FORMAT_VERSION,
			seq: 0,
			ts,
			prev: String::new(),
			kind: RecordKind::Context(ContextRecord {
				sys_user: "felix".into(),
				db_user: "tamanu".into(),
				writemode: false,
				ots: None,
				tailscale: Vec::new(),
				instance,
			}),
		}];
		for seq in 1..4 {
			let prev = super::super::record::hash(&records[seq as usize - 1].to_json().unwrap());
			records.push(Record {
				v: FORMAT_VERSION,
				seq,
				ts,
				prev,
				kind: RecordKind::Query(QueryRecord {
					query: format!("select {seq};"),
					source: QuerySource::Typed,
				}),
			});
		}

		let bytes: Vec<u8> = records
			.iter()
			.flat_map(|record| frame(&record.to_json().unwrap()))
			.collect();
		std::fs::write(
			dir.path().join(paths::segment_name(
				ts.to_zoned(jiff::tz::TimeZone::UTC).date(),
				instance,
			)),
			bytes,
		)
		.unwrap();

		let mut out = Vec::new();
		write_export(
			&mut out,
			dir.path(),
			&QueryOptions {
				limit: Some(2),
				..Default::default()
			},
		)
		.unwrap();

		assert!(
			String::from_utf8(out)
				.unwrap()
				.contains(r#""sys_user":"felix""#),
			"the context in force is emitted even at an identical timestamp"
		);
	}

	#[test]
	fn a_time_range_narrows_the_export() {
		let dir = tempfile::tempdir().unwrap();
		let mut writer = Writer::new(dir.path());
		writer.query(&context(), "before;".into(), QuerySource::Typed);
		std::thread::sleep(std::time::Duration::from_millis(5));
		let cut = Timestamp::now();
		std::thread::sleep(std::time::Duration::from_millis(5));
		writer.query(&context(), "after;".into(), QuerySource::Typed);
		drop(writer);

		let found = entries(
			dir.path(),
			&QueryOptions {
				since: Some(cut.to_string()),
				..Default::default()
			},
		)
		.unwrap();
		assert_eq!(queries(&found), vec!["after;"]);
	}

	#[test]
	fn dates_and_datetimes_parse_as_a_range() {
		assert!(parse_time("2026-09-08").is_ok());
		assert!(parse_time("2026-09-08 03:14:15").is_ok());
		assert!(parse_time("2026-09-08T03:14:15Z").is_ok());
		assert!(parse_time("not a time").is_err());
	}

	#[test]
	fn chain_heads_name_every_session() {
		let dir = tempfile::tempdir().unwrap();
		log_of(dir.path(), 2);
		log_of(dir.path(), 2);

		let heads = chain_heads(dir.path()).unwrap();
		assert_eq!(heads.len(), 2);
		assert!(heads.iter().all(|(_, head)| head.len() == 64));
	}

	#[test]
	fn an_empty_directory_exports_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let mut out = Vec::new();
		write_export(&mut out, dir.path(), &QueryOptions::default()).unwrap();
		assert!(out.is_empty());
	}
}
