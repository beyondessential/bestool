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
				history.set_repl_state(&ctx.repl_state.lock().unwrap());
				if let Err(e) = history.add_entry(edited_content.clone()) {
					debug!("failed to add to history: {e}");
				}

				let (_, action) =
					handle_input("", &edited_content, &ctx.repl_state.lock().unwrap());

				if let ReplAction::Execute {
					input,
					sql,
					modifiers,
				} = action
				{
					return handle_execute(ctx, input, sql, modifiers).await;
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
