use std::ops::ControlFlow;

use super::state::ReplContext;

pub async fn handle_debug(
	ctx: &mut ReplContext<'_>,
	what: crate::parser::DebugWhat,
) -> ControlFlow<()> {
	use crate::parser::DebugWhat;

	match what {
		DebugWhat::State => {
			let state = ctx.repl_state.lock().unwrap();
			eprintln!("ReplState: {:#?}", *state);
		}
		DebugWhat::RefreshSchema => {
			eprintln!("Refreshing schema cache...");
			match ctx.schema_cache_manager.refresh().await {
				Ok(()) => eprintln!("Schema cache refreshed successfully"),
				Err(e) => eprintln!("Failed to refresh schema cache: {e}"),
			}
		}
		DebugWhat::Help => {
			eprintln!("Available debug commands:");
			eprintln!("  \\debug state           - Show current REPL state");
			eprintln!("  \\debug refresh-schema  - Refresh schema cache (for completion)");
		}
	}

	ControlFlow::Continue(())
}
