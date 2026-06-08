use std::{fs, ops::ControlFlow, path::Path};

use tracing::debug;

use super::state::ReplContext;
use crate::input::handle_input;

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

		let saved_vars: Vec<(String, Option<String>)> = {
			let mut state = ctx.repl_state.lock().unwrap();
			state.from_snippet_or_include = true;
			let saved: Vec<(String, Option<String>)> = vars
				.iter()
				.map(|(name, _)| (name.clone(), state.vars.get(name).cloned()))
				.collect();

			for (name, value) in &vars {
				state.vars.insert(name.clone(), value.clone());
			}
			saved
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
		for action in actions {
			// Boxed because an included file may itself include/run another,
			// making dispatch indirectly recursive.
			result = Box::pin(action.dispatch(ctx, "")).await;
			if result.is_break() {
				break;
			}
		}

		{
			let mut state = ctx.repl_state.lock().unwrap();
			state.from_snippet_or_include = false;
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
