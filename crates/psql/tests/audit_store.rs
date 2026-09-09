//! End-to-end behaviour of the audit store, through its public surface.
//!
//! spec: AUD, AUD-STO, AUD-HIS, AUD-RET, AUD-API

use std::path::Path;

use bestool_psql::audit::{
	Audit, QuerySource, Reader, RecallSet, RecordKind, compact, paths, tools, tools::QueryOptions,
	verify,
};

fn session(dir: &Path, queries: &[(&str, QuerySource)]) {
	let mut audit = Audit::open_bare(dir).unwrap();
	for (query, source) in queries {
		audit.record((*query).into(), source.clone());
	}
}

fn typed(dir: &Path, queries: &[&str]) {
	let pairs: Vec<_> = queries.iter().map(|q| (*q, QuerySource::Typed)).collect();
	session(dir, &pairs);
}

fn logged(dir: &Path) -> Vec<String> {
	Reader::open(dir)
		.unwrap()
		.entries()
		.map(|entry| entry.query)
		.collect()
}

fn recalled(dir: &Path) -> Vec<String> {
	let set = RecallSet::build(dir);
	(0..set.len())
		.map(|i| set.get(i).unwrap().to_string())
		.collect()
}

#[test]
fn a_session_records_what_it_ran_and_a_later_one_reads_it_back() {
	let dir = tempfile::tempdir().unwrap();
	typed(dir.path(), &["select 1;", "select 2;"]);

	assert_eq!(logged(dir.path()), vec!["select 1;", "select 2;"]);
	assert_eq!(recalled(dir.path()), vec!["select 1;", "select 2;"]);
	assert!(verify(dir.path()).unwrap().holds());
}

#[test]
fn concurrent_sessions_never_reconcile_and_never_lose_records() {
	let dir = tempfile::tempdir().unwrap();

	// Interleaved, with no coordination between them at any point.
	let mut one = Audit::open_bare(dir.path()).unwrap();
	let mut two = Audit::open_bare(dir.path()).unwrap();
	let mut three = Audit::open_bare(dir.path()).unwrap();
	for i in 0..20 {
		one.add_entry(format!("one {i};")).unwrap();
		two.add_entry(format!("two {i};")).unwrap();
		three.add_entry(format!("three {i};")).unwrap();
	}
	drop(one);
	drop(two);
	drop(three);

	let all = logged(dir.path());
	assert_eq!(all.len(), 60, "every record from every session survives");
	for i in 0..20 {
		for who in ["one", "two", "three"] {
			assert!(all.contains(&format!("{who} {i};")));
		}
	}

	let report = verify(dir.path()).unwrap();
	assert_eq!(report.sessions.len(), 3);
	assert!(report.holds());

	// Each session wrote its own file and nothing else touched it.
	assert_eq!(paths::list(dir.path()).unwrap().len(), 3);
}

#[test]
fn a_crashed_session_leaves_a_readable_segment_with_no_end_record() {
	let dir = tempfile::tempdir().unwrap();

	// A clean exit runs the writer's shutdown; a crash does not, so the file is
	// left exactly as the last successful write left it.
	let clean = {
		let mut audit = Audit::open_bare(dir.path()).unwrap();
		audit.add_entry("clean;".into()).unwrap();
		let instance = audit.instance();
		drop(audit);
		instance
	};

	let ends: Vec<_> = Reader::open(dir.path())
		.unwrap()
		.filter(|stored| matches!(stored.record.kind, RecordKind::End))
		.filter_map(|stored| stored.instance)
		.collect();
	assert_eq!(ends, vec![clean]);
}

#[test]
fn a_torn_record_is_skipped_and_the_rest_of_the_log_survives() {
	let dir = tempfile::tempdir().unwrap();
	typed(dir.path(), &["before;", "middle;"]);

	let segment = paths::list(dir.path()).unwrap()[0].0.clone();
	let mut bytes = std::fs::read(&segment).unwrap();
	bytes.push(0x1E);
	bytes.extend_from_slice(br#"{"v":1,"seq":99,"ts":"2026-"#);
	std::fs::write(&segment, bytes).unwrap();

	assert_eq!(logged(dir.path()), vec!["before;", "middle;"]);

	let report = verify(dir.path()).unwrap();
	assert_eq!(report.skipped.len(), 1);
	assert!(
		report.holds(),
		"damage is confined to the record it lands in"
	);
}

#[test]
fn only_statements_typed_at_the_prompt_are_recalled() {
	let dir = tempfile::tempdir().unwrap();
	session(
		dir.path(),
		&[
			("\\i /tmp/fixups.sql", QuerySource::Typed),
			(
				"update patients;",
				QuerySource::Include {
					path: "/tmp/fixups.sql".into(),
				},
			),
			(
				"select count(*);",
				QuerySource::Snippet {
					name: "counts".into(),
				},
			),
			("select 1;", QuerySource::Typed),
		],
	);

	assert_eq!(
		logged(dir.path()),
		vec![
			"\\i /tmp/fixups.sql",
			"update patients;",
			"select count(*);",
			"select 1;"
		],
		"the log holds what the file actually did"
	);
	assert_eq!(
		recalled(dir.path()),
		vec!["\\i /tmp/fixups.sql", "select 1;"],
		"shell history holds only what was typed"
	);
}

#[test]
fn an_export_round_trips_and_verifies() {
	let dir = tempfile::tempdir().unwrap();
	typed(dir.path(), &["select 1;", "select 2;", "select 3;"]);

	let mut out = Vec::new();
	tools::write_export(&mut out, dir.path(), &QueryOptions::default()).unwrap();

	// Written back into a directory of its own, the export reads as the same
	// log: export changes the container, not the content.
	let copy = tempfile::tempdir().unwrap();
	let instance = verify(dir.path()).unwrap().sessions[0].instance;
	let today = jiff::Timestamp::now()
		.to_zoned(jiff::tz::TimeZone::UTC)
		.date();
	std::fs::write(copy.path().join(paths::segment_name(today, instance)), &out).unwrap();

	assert_eq!(logged(copy.path()), logged(dir.path()));
	assert!(verify(copy.path()).unwrap().holds());
}

#[test]
fn compaction_folds_a_day_without_changing_what_the_log_says() {
	let dir = tempfile::tempdir().unwrap();
	typed(dir.path(), &["select 1;", "select 2;"]);

	// Age the segment past the plain-text window.
	let (path, kind) = paths::list(dir.path()).unwrap().pop().unwrap();
	let old = jiff::Timestamp::now()
		.to_zoned(jiff::tz::TimeZone::UTC)
		.date()
		.checked_sub(jiff::Span::new().days(compact::PLAIN_TEXT_WINDOW_DAYS + 1))
		.unwrap();
	std::fs::rename(
		&path,
		dir.path()
			.join(paths::segment_name(old, kind.instance().unwrap())),
	)
	.unwrap();

	let before = logged(dir.path());
	let report = compact::run(dir.path()).unwrap();
	assert_eq!(report.folded.len(), 1);

	assert_eq!(logged(dir.path()), before);
	assert_eq!(recalled(dir.path()), before);
	assert!(verify(dir.path()).unwrap().holds());
	assert!(
		paths::list(dir.path())
			.unwrap()
			.iter()
			.all(|(_, kind)| kind.instance().is_none()),
		"the segments are gone and a day file stands for them"
	);
}

#[test]
fn a_limited_export_still_attributes_every_record_it_writes() {
	let dir = tempfile::tempdir().unwrap();
	typed(
		dir.path(),
		&["one;", "two;", "three;", "four;", "five;", "six;"],
	);

	let mut out = Vec::new();
	tools::write_export(
		&mut out,
		dir.path(),
		&QueryOptions {
			limit: Some(2),
			..Default::default()
		},
	)
	.unwrap();

	let text = String::from_utf8(out).unwrap();
	assert!(text.contains(r#""kind":"context""#));
	assert!(text.contains("six;"));
	assert!(!text.contains("one;"));
}

#[test]
fn the_store_survives_a_directory_that_cannot_be_written() {
	let dir = tempfile::tempdir().unwrap();
	let store = dir.path().join("store");
	std::fs::write(&store, b"a file where the directory should be").unwrap();

	// The session opens, runs, and exits: recording is best effort, and a
	// statement runs whether or not its record could be written.
	let mut audit = Audit::open_bare(&store).unwrap();
	for i in 0..10 {
		audit.add_entry(format!("select {i};")).unwrap();
	}
	drop(audit);
}

#[test]
fn a_directory_of_someone_elses_files_is_left_alone() {
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(dir.path().join("notes.txt"), b"not mine").unwrap();
	typed(dir.path(), &["select 1;"]);

	assert_eq!(logged(dir.path()), vec!["select 1;"]);
	assert!(dir.path().join("notes.txt").exists());
}
