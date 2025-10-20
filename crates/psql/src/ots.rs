//! OTS (Over The Shoulder) prompting with history support

use crate::history::History;
use miette::{IntoDiagnostic, Result};
use redb::{Database, ReadableDatabase, ReadableTable};
use rustyline::history::{History as HistoryTrait, MemHistory};
use rustyline::{Config, Editor};
use std::path::Path;
use std::sync::{Arc, RwLock};
use tracing::{debug, warn};

/// Prompt for OTS value with rustyline and history from previous OTS values
///
/// This can either open the history database from a path, or use a shared database handle
pub fn prompt_for_ots(history_path: &Path) -> Result<String> {
	prompt_for_ots_with_db(None, Some(history_path))
}

/// Prompt for OTS value with rustyline and history from a shared database handle
pub fn prompt_for_ots_with_db(
	db: Option<Arc<RwLock<Database>>>,
	history_path: Option<&Path>,
) -> Result<String> {
	let ots_history = load_ots_history(db, history_path)?;

	let mut rl: Editor<(), MemHistory> = Editor::with_history(
		Config::builder()
			.auto_add_history(false)
			.history_ignore_dups(true)
			.unwrap()
			.build(),
		MemHistory::new(),
	)
	.into_diagnostic()?;

	for ots_value in ots_history.iter().rev() {
		let _ = rl.history_mut().add(ots_value);
	}

	loop {
		match rl.readline("OTS? ") {
			Ok(line) => {
				let trimmed = line.trim();
				if trimmed.is_empty() {
					eprintln!("OTS is required for write mode");
					continue;
				}
				return Ok(trimmed.to_string());
			}
			Err(rustyline::error::ReadlineError::Interrupted) => {
				return Err(miette::miette!("OTS prompt interrupted"));
			}
			Err(rustyline::error::ReadlineError::Eof) => {
				return Err(miette::miette!("OTS is required for write mode"));
			}
			Err(err) => {
				return Err(err).into_diagnostic();
			}
		}
	}
}

/// Load unique OTS values from the history database
fn load_ots_history(
	db: Option<Arc<RwLock<Database>>>,
	history_path: Option<&Path>,
) -> Result<Vec<String>> {
	let entries = if let Some(db) = db {
		load_entries_from_db(&db)?
	} else if let Some(path) = history_path {
		let history = match History::open(path) {
			Ok(h) => h,
			Err(e) => {
				warn!("could not open history database for OTS values: {}", e);
				return Ok(Vec::new());
			}
		};

		match history.list() {
			Ok(e) => e,
			Err(e) => {
				warn!("could not read history entries for OTS values: {}", e);
				return Ok(Vec::new());
			}
		}
	} else {
		warn!("no database handle or path provided");
		return Ok(Vec::new());
	};

	// Collect unique OTS values (most recent order preserved by using Vec and dedup)
	let mut ots_values = Vec::new();
	let mut seen = std::collections::HashSet::new();
	for (_timestamp, entry) in entries.into_iter().rev() {
		if let Some(ots) = entry.ots {
			if !ots.is_empty() && seen.insert(ots.clone()) {
				ots_values.push(ots);
			}
		}
	}

	debug!(count = ots_values.len(), "loaded OTS history");
	Ok(ots_values)
}

/// Load entries directly from a database handle
fn load_entries_from_db(
	db: &Arc<RwLock<Database>>,
) -> Result<Vec<(u64, crate::history::HistoryEntry)>> {
	use crate::history::{HistoryEntry, HISTORY_TABLE};
	use miette::IntoDiagnostic;

	let read_txn = db.read().unwrap().begin_read().into_diagnostic()?;
	let table = read_txn.open_table(HISTORY_TABLE).into_diagnostic()?;

	let mut entries = Vec::new();
	for item in table.iter().into_diagnostic()? {
		let (timestamp, json) = item.into_diagnostic()?;
		let entry: HistoryEntry = serde_json::from_str(json.value()).into_diagnostic()?;
		entries.push((timestamp.value(), entry));
	}

	Ok(entries)
}
