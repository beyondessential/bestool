use std::ops::ControlFlow;

use tracing::{debug, warn};

use super::state::ReplContext;
use crate::input::{ReplAction, handle_input};

/// What to seed the `\e` editor with.
///
/// Empty by default; when the immediately-preceding command was also `\e`, its
/// resulting buffer (passed here via `last_edit`), so back-to-back `\e` keeps
/// refining the same text. Never the bare `\e` invocation itself.
fn editor_seed(last_edit: Option<&str>) -> String {
	match last_edit {
		Some(prev) if prev.trim() != "\\e" => prev.to_string(),
		_ => String::new(),
	}
}

pub async fn handle_edit(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	use super::execute::handle_execute;

	let initial_content = {
		let state = ctx.repl_state.lock().unwrap();
		editor_seed(state.last_edit_content.as_deref())
	};

	match edit::edit(&initial_content) {
		Ok(edited_content) => {
			let edited_trimmed = edited_content.trim();

			if !edited_trimmed.is_empty() {
				debug!("editor returned content, processing it");

				// Remember it so a follow-up `\e` reopens this buffer.
				ctx.repl_state.lock().unwrap().last_edit_content = Some(edited_content.clone());

				let history = ctx.rl.history_mut();
				if let Err(e) = history.add_entry(edited_content.clone()) {
					debug!("failed to add to history: {e}");
				}

				let (remaining, mut actions) =
					handle_input("", &edited_content, &ctx.repl_state.lock().unwrap());

				// If there's remaining input (incomplete query), auto-execute it by appending a semicolon.
				// This handles both cases:
				// 1. File with only incomplete query (actions empty)
				// 2. File with complete queries followed by incomplete (actions not empty)
				if !remaining.trim().is_empty() {
					let completed = format!("{};", remaining);
					let (_, new_actions) =
						handle_input("", &completed, &ctx.repl_state.lock().unwrap());
					actions.extend(new_actions);
				}

				for action in actions {
					if let ReplAction::Execute {
						input,
						sql,
						modifiers,
					} = action
					{
						let flow = handle_execute(ctx, input, sql, modifiers).await;
						if flow.is_break() {
							return flow;
						}
					}
				}
			} else {
				debug!("editor returned empty content, skipping");
				// The edit produced nothing, so a follow-up `\e` starts empty.
				ctx.repl_state.lock().unwrap().last_edit_content = None;
			}
		}
		Err(e) => {
			warn!("editor failed: {e}");
		}
	}

	ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
	use super::editor_seed;

	#[test]
	fn seed_empty_by_default() {
		assert_eq!(editor_seed(None), "");
	}

	#[test]
	fn seed_reuses_previous_edit() {
		assert_eq!(editor_seed(Some("SELECT 1")), "SELECT 1");
	}

	#[test]
	fn seed_never_the_edit_metacommand() {
		assert_eq!(editor_seed(Some("\\e")), "");
		assert_eq!(editor_seed(Some("  \\e  ")), "");
	}
}
