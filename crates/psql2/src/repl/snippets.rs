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
			Ok(path) => path,
			Err(err) => {
				tracing::error!("Failed to find snippet '{name}': {err}");
				return ControlFlow::Continue(());
			}
		}
	};

	handle_include(ctx, &file_path, vars).await
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
	history.set_repl_state(&ctx.repl_state.lock().unwrap());
	if let Err(e) = history.add_entry(line.into()) {
		debug!("failed to add SnippetSave to history: {e}");
	}

	ControlFlow::Continue(())
}
