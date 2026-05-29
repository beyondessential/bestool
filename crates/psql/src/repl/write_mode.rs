use std::{
	ops::ControlFlow,
	sync::{Arc, Mutex},
	time::{Duration, Instant},
};

use bestool_postgres::pool::PgConnection;
use rustyline::ExternalPrinter;
use tracing::{debug, error, warn};

use super::{ReplState, state::ReplContext, transaction::TransactionState};
use crate::ots;

/// How long write mode can stay enabled without activity before it is
/// automatically reverted to read-only.
pub const WRITE_MODE_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How often the watcher wakes up to check whether write mode should expire.
pub const WRITE_MODE_TIMEOUT_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Bump the "active in write mode" timestamp so the idle-timeout clock resets.
pub fn mark_write_mode_active(repl_state: &Arc<Mutex<ReplState>>) {
	let mut state = repl_state.lock().unwrap();
	if state.write_mode {
		state.write_mode_active_at = Some(Instant::now());
	}
}

/// Pure decision used by the watcher: should write mode be timed out, given
/// the current state and observed transaction state?
fn should_time_out_write_mode(
	in_write_mode: bool,
	active_at: Option<Instant>,
	now: Instant,
	tx_state: TransactionState,
	idle_threshold: Duration,
) -> bool {
	if !in_write_mode {
		return false;
	}
	let Some(active_at) = active_at else {
		return false;
	};
	if now.duration_since(active_at) < idle_threshold {
		return false;
	}
	matches!(tx_state, TransactionState::Idle | TransactionState::None)
}

pub async fn handle_write_mode_toggle(
	ctx: &mut ReplContext<'_>,
	ots: Option<String>,
) -> ControlFlow<()> {
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
		new_state.write_mode_active_at = None;

		match ctx
			.client
			.batch_execute("ROLLBACK; SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
			.await
		{
			Ok(_) => {
				debug!("Write mode disabled");
				eprintln!("SESSION IS NOW READ ONLY");
				*ctx.repl_state.lock().unwrap() = new_state;
			}
			Err(e) => {
				error!("Failed to disable write mode: {e}");
			}
		}
	} else {
		// A trimmed, non-empty argument is recorded directly as the OTS; an
		// empty or absent argument falls back to the interactive prompt. Either
		// way the value flows into `new_state.ots`, so the audit records it
		// identically to the prompted path.
		let supplied_ots = ots.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
		let new_ots = match supplied_ots {
			Some(ots) => Ok(ots),
			None => ots::prompt_for_ots(ctx.rl.history()),
		};
		match new_ots {
			Ok(new_ots) => {
				let mut new_state = state.clone();
				new_state.write_mode = true;
				new_state.ots = Some(new_ots.clone());
				new_state.write_mode_active_at = Some(Instant::now());

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

/// Background watcher that reverts write mode to read-only after
/// `WRITE_MODE_IDLE_TIMEOUT` of inactivity, provided the session isn't
/// holding an uncommitted transaction.
///
/// Designed to be `tokio::spawn`ed for the lifetime of the REPL; aborting the
/// returned task on shutdown is fine — there is no shared state to flush.
pub async fn watch_write_mode_idle_timeout<P: ExternalPrinter + Send + 'static>(
	client: Arc<PgConnection>,
	monitor_client: Arc<PgConnection>,
	backend_pid: i32,
	repl_state: Arc<Mutex<ReplState>>,
	mut printer: P,
) {
	loop {
		tokio::time::sleep(WRITE_MODE_TIMEOUT_CHECK_INTERVAL).await;

		let (in_write_mode, active_at) = {
			let state = repl_state.lock().unwrap();
			(state.write_mode, state.write_mode_active_at)
		};

		if !in_write_mode {
			continue;
		}
		let Some(active_at) = active_at else { continue };
		if active_at.elapsed() < WRITE_MODE_IDLE_TIMEOUT {
			continue;
		}

		// Don't yank write mode out from under work that's mid-flight or has
		// uncommitted changes. The watcher will try again on its next tick.
		let tx_state = TransactionState::check(&monitor_client, backend_pid).await;
		if !should_time_out_write_mode(
			in_write_mode,
			Some(active_at),
			Instant::now(),
			tx_state,
			WRITE_MODE_IDLE_TIMEOUT,
		) {
			continue;
		}

		match client
			.batch_execute("ROLLBACK; SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
			.await
		{
			Ok(_) => {
				{
					let mut state = repl_state.lock().unwrap();
					// Recheck under the lock: an interactive toggle may have raced us.
					if !state.write_mode {
						continue;
					}
					state.write_mode = false;
					state.ots = None;
					state.write_mode_active_at = None;
				}
				let minutes = WRITE_MODE_IDLE_TIMEOUT.as_secs() / 60;
				let msg = format!(
					"\nWrite mode idle for {minutes} minutes — session reverted to read-only."
				);
				if let Err(e) = printer.print(msg) {
					warn!("failed to print write-mode timeout notice: {e}");
				}
				debug!("write mode timed out due to inactivity");
			}
			Err(e) => {
				warn!("failed to revert write mode on timeout: {e}");
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// `Instant - Duration` panics on Windows when the system has been up
	// for less than the duration (e.g. fresh CI runners); construct the
	// "later" instant by adding instead.
	fn baseline() -> (Instant, Instant) {
		let earlier = Instant::now();
		let later = earlier + Duration::from_secs(3600);
		(earlier, later)
	}

	#[test]
	fn does_not_time_out_when_write_mode_off() {
		let (earlier, later) = baseline();
		assert!(!should_time_out_write_mode(
			false,
			Some(earlier),
			later,
			TransactionState::None,
			Duration::from_secs(600),
		));
	}

	#[test]
	fn does_not_time_out_when_no_activity_recorded() {
		let (_, later) = baseline();
		assert!(!should_time_out_write_mode(
			true,
			None,
			later,
			TransactionState::None,
			Duration::from_secs(600),
		));
	}

	#[test]
	fn does_not_time_out_before_threshold() {
		let earlier = Instant::now();
		let later = earlier + Duration::from_secs(60);
		assert!(!should_time_out_write_mode(
			true,
			Some(earlier),
			later,
			TransactionState::None,
			Duration::from_secs(600),
		));
	}

	#[test]
	fn does_not_time_out_with_uncommitted_writes() {
		let (earlier, later) = baseline();
		// An open transaction with pending xid (Active) blocks the timeout
		// so the watcher doesn't rollback work in progress.
		assert!(!should_time_out_write_mode(
			true,
			Some(earlier),
			later,
			TransactionState::Active,
			Duration::from_secs(600),
		));
		assert!(!should_time_out_write_mode(
			true,
			Some(earlier),
			later,
			TransactionState::Error,
			Duration::from_secs(600),
		));
	}

	#[test]
	fn times_out_when_idle_and_past_threshold() {
		let earlier = Instant::now();
		let later = earlier + Duration::from_secs(601);
		assert!(should_time_out_write_mode(
			true,
			Some(earlier),
			later,
			TransactionState::Idle,
			Duration::from_secs(600),
		));
		assert!(should_time_out_write_mode(
			true,
			Some(earlier),
			later,
			TransactionState::None,
			Duration::from_secs(600),
		));
	}

	#[test]
	fn mark_write_mode_active_only_runs_when_in_write_mode() {
		let state = Arc::new(Mutex::new(ReplState::new()));

		// Not in write mode: timestamp stays None.
		mark_write_mode_active(&state);
		assert!(state.lock().unwrap().write_mode_active_at.is_none());

		// Now in write mode: timestamp gets populated.
		state.lock().unwrap().write_mode = true;
		mark_write_mode_active(&state);
		assert!(state.lock().unwrap().write_mode_active_at.is_some());
	}
}
