//! Wiring between [`bestool_winsvc`] and the `rdp monitor` async loop.
//!
//! When `bestool.exe rdp monitor --service` is launched by the Service
//! Control Manager, `main.rs` intercepts that command line *before* building
//! the tokio runtime and calls [`dispatch`]. The SCM handshake runs on the
//! current thread; our entry function (on the service thread) builds a
//! single-threaded tokio runtime and drives the normal monitor loop until the
//! SCM signals stop.

use std::time::Duration;

use miette::{IntoDiagnostic, Result, WrapErr};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::{
	audit::AuditLog,
	events::poll_events,
	monitor::MonitorArgs,
	service::SERVICE_NAME,
	state::Tracker,
};

/// Entry point for service-mode execution. Blocks until the SCM signals stop.
///
/// This function expects to be called from `main()` *before* any tokio runtime
/// exists. Inside the service thread started by the SCM it constructs its own
/// runtime and awaits shutdown.
pub fn dispatch(args: MonitorArgs) -> Result<()> {
	ARGS.with(|cell| *cell.borrow_mut() = Some(args));
	bestool_winsvc::run(SERVICE_NAME, service_entry)
		.map_err(|e| miette::miette!("service dispatcher failed: {e}"))
}

thread_local! {
	static ARGS: std::cell::RefCell<Option<MonitorArgs>> = const { std::cell::RefCell::new(None) };
}

fn service_entry(shutdown: watch::Receiver<bool>) {
	let args = match ARGS.with(|cell| cell.borrow_mut().take()) {
		Some(a) => a,
		None => return,
	};

	let rt = match tokio::runtime::Builder::new_current_thread()
		.enable_all()
		.build()
	{
		Ok(rt) => rt,
		Err(err) => {
			warn!(?err, "failed to build tokio runtime for service");
			return;
		}
	};

	rt.block_on(async move {
		if let Err(err) = run_loop(args, shutdown).await {
			warn!(?err, "service loop exited with error");
		}
	});
}

async fn run_loop(args: MonitorArgs, mut shutdown: watch::Receiver<bool>) -> Result<()> {
	let mut audit = AuditLog::open(&args.audit_log)
		.await
		.wrap_err("opening audit log")?;
	let mut tracker = Tracker::new(Duration::from_secs(args.kick_window));
	let mut since = chrono::Utc::now() - chrono::Duration::seconds(args.poll_interval as i64);
	let mut last_record_id: u64 = 0;
	let mut interval = tokio::time::interval(Duration::from_secs(args.poll_interval));
	interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

	info!(service = SERVICE_NAME, "service monitor loop started");

	loop {
		tokio::select! {
			_ = interval.tick() => {
				let now = chrono::Utc::now();
				match poll_events(since).await {
					Ok(events) => {
						since = now;
						for ev in events {
							if ev.record_id <= last_record_id { continue; }
							last_record_id = ev.record_id;
							super::monitor::handle_event(ev, &mut tracker, &mut audit, args.tailscale_only).await;
						}
					}
					Err(err) => warn!(?err, "failed to poll event log; will retry"),
				}
			}
			changed = shutdown.changed() => {
				changed.into_diagnostic().wrap_err("shutdown channel closed")?;
				if *shutdown.borrow() {
					debug!("shutdown signalled; exiting monitor loop");
					break;
				}
			}
		}
	}

	Ok(())
}
