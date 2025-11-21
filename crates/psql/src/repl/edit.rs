use std::ops::ControlFlow;

use rustyline::history::{History, SearchDirection};
use tracing::{debug, warn};

use super::state::ReplContext;
use crate::input::{ReplAction, handle_input};

pub async fn handle_edit(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	use super::execute::handle_execute;

	let initial_content = {
		let hist_len = ctx.rl.history().len();
		if hist_len > 0 {
			match ctx.rl.history().get(hist_len - 1, SearchDirection::Forward) {
				Ok(Some(result)) => result.entry.to_string(),
				_ => String::new(),
			}
		} else {
			String::new()
		}
	};

	match edit::edit(&initial_content) {
		Ok(edited_content) => {
			let edited_trimmed = edited_content.trim();

			if !edited_trimmed.is_empty() {
				debug!("editor returned content, processing it");

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
			}
		}
		Err(e) => {
			warn!("editor failed: {e}");
		}
	}

	ControlFlow::Continue(())
}
