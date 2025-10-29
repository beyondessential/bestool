use std::ops::ControlFlow;

pub fn handle_exit() -> ControlFlow<()> {
	ControlFlow::Break(())
}
