//! OTS (Over The Shoulder) prompting with history support

use miette::{IntoDiagnostic, Result};
use rustyline::history::{History as HistoryTrait, MemHistory};
use rustyline::{Config, Editor};
use std::path::Path;
use tracing::{debug, warn};

/// Prompt for OTS value with rustyline and history from previous OTS values
pub fn prompt_for_ots(history_path: &Path) -> Result<String> {
	let ots_history = load_ots_history(history_path)?;

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
fn load_ots_history(history_path: &Path) -> Result<Vec<String>> {
	use crate::history::History;

	let history = match History::open(history_path) {
		Ok(h) => h,
		Err(e) => {
			warn!("could not open history database for OTS values: {}", e);
			return Ok(Vec::new());
		}
	};

	let entries = match history.list() {
		Ok(e) => e,
		Err(e) => {
			warn!("could not read history entries for OTS values: {}", e);
			return Ok(Vec::new());
		}
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
