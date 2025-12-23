use std::ops::ControlFlow;

use rustyline::history::History;
use tracing::debug;

use super::state::ReplContext;

pub async fn handle_run_snippet(
	ctx: &mut ReplContext<'_>,
	name: String,
	vars: Vec<(String, String)>,
) -> ControlFlow<()> {
	use super::include::handle_include;

	let file_path = {
		let state = ctx.repl_state.lock().unwrap();
		match state.snippets.path(&name) {
			Ok(path) => Some(path),
			Err(_) => None,
		}
	};

	if let Some(file_path) = file_path {
		return handle_include(ctx, &file_path, vars).await;
	}

	let lookup_content = {
		let state = ctx.repl_state.lock().unwrap();
		if let Some(lookup_provider) = &state.config.snippet_lookup {
			lookup_provider.lookup(&name)
		} else {
			None
		}
	};

	match lookup_content {
		Some(content) => {
			use crate::input::{ReplAction, handle_input};

			let history = ctx.rl.history_mut();
			if let Err(e) = history.add_entry(content.clone()) {
				tracing::debug!("failed to add to history: {e}");
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

			let (remaining, mut actions) =
				handle_input("", &content, &ctx.repl_state.lock().unwrap());

			if !remaining.trim().is_empty() {
				let completed = format!("{};", remaining);
				let (_, new_actions) =
					handle_input("", &completed, &ctx.repl_state.lock().unwrap());
				actions.extend(new_actions);
			}

			let mut result = ControlFlow::Continue(());
			for action in actions {
				if let ReplAction::Execute {
					input,
					sql,
					modifiers,
				} = action
				{
					use super::execute::handle_execute;
					result = handle_execute(ctx, input, sql, modifiers).await;
					if result.is_break() {
						break;
					}
				}
			}

			{
				let mut state = ctx.repl_state.lock().unwrap();
				for (name, original_value) in saved_vars {
					match original_value {
						Some(value) => state.vars.insert(name, value),
						None => state.vars.remove(&name),
					};
				}
			}

			result
		}
		None => {
			tracing::error!("Failed to find snippet '{name}'");
			ControlFlow::Continue(())
		}
	}
}

pub async fn handle_snippet_save(
	ctx: &mut ReplContext<'_>,
	name: String,
	line: &str,
) -> ControlFlow<()> {
	let history = ctx.rl.history();

	if history.is_empty() {
		eprintln!("No command history available");
	} else {
		let last_idx = history.len() - 1;
		let content = match history.get(last_idx, rustyline::history::SearchDirection::Forward) {
			Ok(Some(result)) => result.entry.to_string(),
			_ => {
				eprintln!("Failed to retrieve last command from history");
				String::new()
			}
		};

		if !content.is_empty() {
			let snippets = ctx.repl_state.lock().unwrap().snippets.clone();
			match snippets.save(&name, &content).await {
				Ok(path) => {
					println!("Snippet saved to {}", path.display());
				}
				Err(e) => eprintln!("Failed to save snippet '{name}': {e}"),
			}
		}
	}

	let history = ctx.rl.history_mut();
	if let Err(e) = history.add_entry(line.into()) {
		debug!("failed to add SnippetSave to history: {e}");
	}

	ControlFlow::Continue(())
}
