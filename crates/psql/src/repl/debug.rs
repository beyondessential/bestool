use std::ops::ControlFlow;

use super::state::ReplContext;

pub fn handle_debug(ctx: &mut ReplContext<'_>, what: crate::parser::DebugWhat) -> ControlFlow<()> {
	use crate::parser::DebugWhat;

	match what {
		DebugWhat::State => {
			let state = ctx.repl_state.lock().unwrap();
			eprintln!("ReplState: {:#?}", *state);
		}
		DebugWhat::Help => {
			eprintln!("Available debug commands:");
			eprintln!("  \\debug state  - Show current REPL state");
		}
	}

	ControlFlow::Continue(())
}
