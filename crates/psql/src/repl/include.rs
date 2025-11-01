use std::{fs, ops::ControlFlow, path::Path};

use tracing::debug;

use super::state::ReplContext;
use crate::input::{ReplAction, handle_input};

pub async fn handle_include(
	ctx: &mut ReplContext<'_>,
	file_path: &Path,
	vars: Vec<(String, String)>,
) -> ControlFlow<()> {
	use super::execute::handle_execute;

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

		let history = ctx.rl.history_mut();
		history.set_repl_state(&ctx.repl_state.lock().unwrap());
		if let Err(e) = history.add_entry(content.clone()) {
			debug!("failed to add to history: {e}");
		}

		let saved_vars: Vec<(String, Option<String>)> = {
			let mut state = ctx.repl_state.lock().unwrap();
			let saved: Vec<(String, Option<String>)> = vars
				.iter()
				.map(|(name, _)| (name.clone(), state.vars.get(name).cloned()))
				.collect();

			for (name, value) in &vars {
				state.vars.insert(name.clone(), value.clone());
			}
			saved
		};

		let (_, action) = handle_input("", &content, &ctx.repl_state.lock().unwrap());

		let result = if let ReplAction::Execute {
			input,
			sql,
			modifiers,
		} = action
		{
			handle_execute(ctx, input, sql, modifiers).await
		} else {
			ControlFlow::Continue(())
		};

		{
			let mut state = ctx.repl_state.lock().unwrap();
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
