use std::{fs, ops::ControlFlow, path::Path};

use tracing::debug;

use super::state::ReplContext;
use crate::{audit::QuerySource, input::handle_input};

/// Record a statement an expansion ran, under the source currently in force.
pub(super) fn record(ctx: &mut ReplContext<'_>, text: String) {
	let source = ctx.repl_state.lock().unwrap().statement_source.clone();
	ctx.rl.history_mut().record(text, source);
}

pub async fn handle_include(
	ctx: &mut ReplContext<'_>,
	file_path: &Path,
	vars: Vec<(String, String)>,
) -> ControlFlow<()> {
	let content = match fs::read_to_string(file_path) {
		Ok(content) => content,
		Err(e) => {
			tracing::error!("Failed to read file '{file_path:?}': {e}");
			return ControlFlow::Continue(());
		}
	};

	let content_trimmed = content.trim();

	if !content_trimmed.is_empty() {
		debug!("read {} bytes from file '{file_path:?}'", content.len());

		// The file is named in the log by the absolute path it was resolved to,
		// so a record says which file actually ran, not what was typed to reach
		// it.
		let source = QuerySource::Include {
			path: std::fs::canonicalize(file_path)
				.unwrap_or_else(|_| file_path.to_path_buf())
				.display()
				.to_string(),
		};

		let (saved_vars, outer_source) = {
			let mut state = ctx.repl_state.lock().unwrap();
			let outer_source = std::mem::replace(&mut state.statement_source, source);
			let saved: Vec<(String, Option<String>)> = vars
				.iter()
				.map(|(name, _)| (name.clone(), state.vars.get(name).cloned()))
				.collect();

			for (name, value) in &vars {
				state.vars.insert(name.clone(), value.clone());
			}
			(saved, outer_source)
		};

		let (remaining, mut actions) = handle_input("", &content, &ctx.repl_state.lock().unwrap());

		// If there's remaining input (incomplete query), auto-execute it by appending a semicolon.
		// This handles both cases:
		// 1. File with only incomplete query (actions empty)
		// 2. File with complete queries followed by incomplete (actions not empty)
		if !remaining.trim().is_empty() {
			let completed = format!("{};", remaining);
			let (_, new_actions) = handle_input("", &completed, &ctx.repl_state.lock().unwrap());
			actions.extend(new_actions);
		}

		let mut result = ControlFlow::Continue(());
		for statement in actions {
			// What a file runs is recorded like anything else, so the log holds
			// what the file actually did rather than only the line that
			// included it.
			record(ctx, statement.text);

			// Boxed because an included file may itself include/run another,
			// making dispatch indirectly recursive.
			result = Box::pin(statement.action.dispatch(ctx, "")).await;
			if result.is_break() {
				break;
			}
		}

		{
			let mut state = ctx.repl_state.lock().unwrap();
			state.statement_source = outer_source;
			for (name, original_value) in saved_vars {
				match original_value {
					Some(value) => state.vars.insert(name, value),
					None => state.vars.remove(&name),
				};
			}
		}

		return result;
	} else {
		debug!("file '{file_path:?}' is empty, skipping");
	}

	ControlFlow::Continue(())
}
