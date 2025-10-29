use std::ops::ControlFlow;

use tracing::{debug, error};

use super::{state::ReplContext, transaction::TransactionState};
use crate::ots;

pub async fn handle_write_mode_toggle(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	let state = { ctx.repl_state.lock().unwrap().clone() };

	if state.write_mode {
		let tx_state = TransactionState::check(ctx.monitor_client, ctx.backend_pid).await;
		if !matches!(tx_state, TransactionState::Idle | TransactionState::None) {
			eprintln!(
				"Cannot disable write mode while in a transaction. COMMIT or ROLLBACK first."
			);
			return ControlFlow::Continue(());
		}

		let mut new_state = state.clone();
		new_state.write_mode = false;
		new_state.ots = None;

		match ctx
			.client
			.batch_execute("ROLLBACK; SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
			.await
		{
			Ok(_) => {
				debug!("Write mode disabled");
				eprintln!("SESSION IS NOW READ ONLY");
				ctx.rl.history_mut().set_repl_state(&new_state);
				*ctx.repl_state.lock().unwrap() = new_state;
			}
			Err(e) => {
				error!("Failed to disable write mode: {e}");
			}
		}
	} else {
		match ots::prompt_for_ots(ctx.rl.history()) {
			Ok(new_ots) => {
				let mut new_state = state.clone();
				new_state.write_mode = true;
				new_state.ots = Some(new_ots.clone());

				match ctx
					.client
					.batch_execute(
						"SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; COMMIT; BEGIN",
					)
					.await
				{
					Ok(_) => {
						debug!("Write mode enabled");
						eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");
						ctx.rl.history_mut().set_repl_state(&new_state);
						*ctx.repl_state.lock().unwrap() = new_state;
					}
					Err(e) => {
						error!("Failed to enable write mode: {e}");
					}
				}
			}
			Err(e) => {
				error!("Failed to enable write mode: {e}");
			}
		}
	}

	ControlFlow::Continue(())
}
