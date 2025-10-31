use std::ops::ControlFlow;

use super::state::ReplContext;

pub fn handle_toggle_expanded(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	let mut state = ctx.repl_state.lock().unwrap();
	state.expanded_mode = !state.expanded_mode;
	eprintln!(
		"Expanded display is {}.",
		if state.expanded_mode { "on" } else { "off" }
	);
	ControlFlow::Continue(())
}
