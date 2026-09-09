//! The audit log.
//!
//! bestool-psql records every statement a session runs into a local append-only
//! log, together with who ran it and under what conditions. The log serves two
//! readers with different needs: the session itself, which recalls recent
//! statements as shell history, and an auditor, who later asks what was run on a
//! machine, by whom, and when.
//!
//! spec: AUD

use std::{
	path::{Path, PathBuf},
	sync::{Arc, Mutex},
};

use miette::Result;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::repl::ReplState;

#[cfg(feature = "cli")]
pub mod cli;
pub mod compact;
pub mod history;
mod legacy;
mod lock;
pub mod paths;
pub mod read;
pub mod record;
mod tailscale;
pub mod tools;
pub mod verify;
mod writer;

pub use history::RecallSet;
pub use read::{Entry, Range, Reader, Stored};
pub use record::{QuerySource, Record, RecordKind};
pub use tools::{ExportOptions, QueryOptions, export_audit_entries};
pub use verify::{VerifyReport, verify};
pub use writer::Writer;

/// A session's handle on the audit log: what it writes, and what it recalls.
#[derive(Debug)]
pub struct Audit {
	writer: Writer,
	recall: RecallSet,
	/// State to record as context for new records.
	repl_state: Arc<Mutex<ReplState>>,
}

impl Audit {
	/// Open the log for a session.
	///
	/// Builds the recall set from what is already recorded, then starts
	/// compaction in the background. Neither the read nor the compaction stands
	/// between the operator and their first prompt.
	pub fn open(dir: impl AsRef<Path>, repl_state: Arc<Mutex<ReplState>>) -> Result<Self> {
		let dir = dir.as_ref();
		debug!(?dir, "opening audit log");

		// Opening never stands between an operator and their prompt: a store
		// that cannot be read yet still gives a session that runs, and the
		// writer keeps trying as it goes.
		if let Err(err) = paths::create_dir(dir) {
			warn!(?err, ?dir, "cannot prepare the audit directory");
		}

		// A store still in the old single-file format is brought across before
		// anything reads it, so the recall set sees what it held.
		match lock::Lock::try_directory(dir) {
			Ok(Some(lock)) => {
				if let Err(err) = legacy::import(dir, &lock) {
					warn!(?err, "could not import the legacy audit store");
				}
			}
			Ok(None) => debug!("another process holds the audit directory"),
			Err(err) => warn!(?err, "could not take the audit directory lock"),
		}

		let recall = RecallSet::build(dir);
		compact::spawn(dir.to_path_buf());

		Ok(Self {
			writer: Writer::new(dir),
			recall,
			repl_state,
		})
	}

	/// Open the log without a REPL behind it, for tests and tools that write.
	pub fn open_bare(dir: impl AsRef<Path>) -> Result<Self> {
		Self::open(dir, Arc::new(Mutex::new(ReplState::new())))
	}

	/// This session's identity, which its records are keyed by.
	pub fn instance(&self) -> Uuid {
		self.writer.instance()
	}

	/// Record a statement typed at the prompt.
	pub fn add_entry(&mut self, query: String) -> Result<()> {
		self.record(query, QuerySource::Typed);
		Ok(())
	}

	/// Record a statement, saying where it came from.
	///
	/// Statements a snippet or an included file ran are recorded like any other,
	/// so the log holds what a file actually did rather than only the line that
	/// invoked it, and they are kept out of shell history.
	pub fn record(&mut self, query: String, source: QuerySource) {
		if source.is_recallable() {
			self.recall.push(query.clone());
		}
		let context = self.context();
		self.writer.query(&context, query, source);
	}

	/// Supervisors named in the log, newest first, for the write-mode prompt.
	pub fn supervisors(&self) -> &[String] {
		self.recall.supervisors()
	}

	fn context(&self) -> writer::Context {
		let state = self.repl_state.lock().unwrap();
		writer::Context {
			sys_user: state.sys_user.clone(),
			db_user: state.db_user.clone(),
			writemode: state.write_mode,
			ots: state.ots.clone(),
		}
	}
}

/// The audit directory a session uses by default.
pub fn default_path() -> Result<PathBuf> {
	paths::default_dir()
}

/// The default audit directory, for help text.
pub fn default_path_for_help() -> String {
	paths::default_dir_for_help()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_session_records_and_recalls_what_it_typed() {
		let dir = tempfile::tempdir().unwrap();
		let mut audit = Audit::open_bare(dir.path()).unwrap();
		audit.add_entry("select 1;".into()).unwrap();
		audit.add_entry("select 2;".into()).unwrap();

		assert_eq!(audit.recall.len(), 2);
		assert_eq!(audit.recall.get(0), Some("select 1;"));
		assert_eq!(audit.recall.get(1), Some("select 2;"));
	}

	#[test]
	fn statements_a_file_ran_are_recorded_but_not_recalled() {
		let dir = tempfile::tempdir().unwrap();
		let mut audit = Audit::open_bare(dir.path()).unwrap();
		audit.record("\\i fixups.sql".into(), QuerySource::Typed);
		audit.record(
			"update patients;".into(),
			QuerySource::Include {
				path: "/tmp/fixups.sql".into(),
			},
		);
		let instance = audit.instance();
		drop(audit);

		let logged: Vec<_> = Reader::open(dir.path())
			.unwrap()
			.entries()
			.map(|e| e.query)
			.collect();
		assert_eq!(logged, vec!["\\i fixups.sql", "update patients;"]);
		assert_eq!(RecallSet::build(dir.path()).len(), 1);
		assert!(verify(dir.path()).unwrap().holds());
		assert_eq!(verify(dir.path()).unwrap().sessions[0].instance, instance);
	}

	#[test]
	fn a_new_session_recalls_what_earlier_ones_recorded() {
		let dir = tempfile::tempdir().unwrap();
		let mut first = Audit::open_bare(dir.path()).unwrap();
		first.add_entry("from the first;".into()).unwrap();
		drop(first);

		let second = Audit::open_bare(dir.path()).unwrap();
		assert_eq!(second.recall.get(0), Some("from the first;"));
	}

	#[test]
	fn a_session_does_not_see_what_a_concurrent_one_runs() {
		let dir = tempfile::tempdir().unwrap();
		let mut mine = Audit::open_bare(dir.path()).unwrap();
		mine.add_entry("mine;".into()).unwrap();

		let mut theirs = Audit::open_bare(dir.path()).unwrap();
		theirs.add_entry("theirs;".into()).unwrap();

		// Pressing up shows what I just ran, never what someone else did in the
		// meantime.
		assert_eq!(mine.recall.len(), 1);
		assert_eq!(mine.recall.get(0), Some("mine;"));
	}

	#[test]
	fn a_new_session_sees_records_from_one_that_was_live_at_the_time() {
		let dir = tempfile::tempdir().unwrap();
		let mut live = Audit::open_bare(dir.path()).unwrap();
		live.add_entry("from the live one;".into()).unwrap();

		let later = Audit::open_bare(dir.path()).unwrap();
		assert_eq!(later.recall.get(0), Some("from the live one;"));
	}
}
