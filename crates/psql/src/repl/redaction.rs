use std::ops::ControlFlow;

use super::state::ReplContext;

pub fn handle_toggle_redaction(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	let mut state = ctx.repl_state.lock().unwrap();

	// Only toggle if redactions are available
	if state.config.redactions.is_empty() {
		eprintln!("Redaction mode is not available (no redactions configured).");
		return ControlFlow::Continue(());
	}

	state.redact_mode = !state.redact_mode;
	eprintln!(
		"Redaction mode is {}.",
		if state.redact_mode { "on" } else { "off" }
	);
	ControlFlow::Continue(())
}
