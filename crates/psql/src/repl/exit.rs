use std::ops::ControlFlow;

use super::{state::ReplContext, transaction::TransactionState};

pub async fn handle_exit(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	let write_mode = {
		let state = ctx.repl_state.lock().unwrap();
		state.write_mode
	};

	if write_mode {
		let tx_state = TransactionState::check(ctx.monitor_client, ctx.backend_pid).await;
		if matches!(tx_state, TransactionState::Active) {
			eprintln!("Cannot exit while in an active transaction. COMMIT or ROLLBACK first.");
			return ControlFlow::Continue(());
		}
	}

	ControlFlow::Break(())
}
