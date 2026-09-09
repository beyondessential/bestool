use miette::{IntoDiagnostic, Result};
use rustyline::{
	Config, Editor,
	history::{History as HistoryTrait, MemHistory},
};

use crate::audit::Audit;

/// Prompt for an over-the-shoulder supervisor, recalling the ones named before.
///
/// The names come from the context records the session already read to build
/// its recall set, so naming a supervisor never waits on the store.
pub fn prompt_for_ots(audit: &Audit) -> Result<String> {
	let ots_history = audit.supervisors();

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
