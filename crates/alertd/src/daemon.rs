use std::{sync::Arc, time::Duration};

use miette::{Result, miette};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};

use crate::{
	DaemonConfig, LogError, canopy::CanopyClient, context::InternalContext, http_server, metrics,
	tasks::TaskContext,
};

enum DaemonEvent {
	/// Clean stop (SIGINT/SIGTERM, or the service manager): exit 0, no restart.
	Shutdown,
	/// Exit non-zero so the service manager (systemd `Restart=`, Windows SCM
	/// recovery) brings the daemon back — how `bestool alertd restart` works.
	Restart,
	WatchdogTimeout,
}

/// Handle the HTTP control endpoints use to drive the daemon.
///
/// `pub` only so it can appear in the (also-internal) `ServerState` /
/// `start_server` signatures without tripping `private_interfaces`; the
/// enclosing `daemon` module is private, so it isn't part of the public API.
#[derive(Clone)]
pub struct DaemonControl {
	reload: Arc<tokio::sync::watch::Sender<u64>>,
	events: mpsc::Sender<DaemonEvent>,
}

impl DaemonControl {
	/// Bump the reload channel so tasks refresh (HTTP `/reload`).
	pub(crate) fn reload(&self) {
		self.reload.send_modify(|n| *n = n.wrapping_add(1));
	}

	/// Ask the daemon to exit so the service manager restarts it (HTTP `/restart`).
	pub(crate) async fn request_restart(&self) {
		let _ = self.events.send(DaemonEvent::Restart).await;
	}

	/// A detached control whose channels go nowhere, for tests.
	#[cfg(test)]
	pub(crate) fn detached() -> Self {
		let (reload, _) = tokio::sync::watch::channel(0);
		let (events, _) = mpsc::channel(1);
		Self {
			reload: Arc::new(reload),
			events,
		}
	}
}

/// A handle a background task can use to ask the daemon to restart itself.
///
/// Held by [`TaskContext`](crate::tasks::TaskContext) so a task that has
/// replaced the running binary (self-update) can have the daemon exit for the
/// service manager to relaunch the new binary, via the same path as the
/// `/restart` control.
#[derive(Clone, Debug)]
pub struct RestartTrigger {
	events: mpsc::Sender<DaemonEvent>,
}

impl RestartTrigger {
	/// Ask the daemon to exit so the service manager restarts it.
	pub async fn request_restart(&self) {
		let _ = self.events.send(DaemonEvent::Restart).await;
	}
}

pub async fn run(daemon_config: DaemonConfig) -> Result<()> {
	let (_shutdown_tx, shutdown_rx) = oneshot::channel();
	run_with_shutdown(daemon_config, shutdown_rx).await
}

pub async fn run_with_shutdown(
	daemon_config: DaemonConfig,
	external_shutdown: oneshot::Receiver<()>,
) -> Result<()> {
	info!("starting alertd daemon");

	// Tie spawned children (pg_basebackup, kopia) to this process, so a daemon
	// restart can't leave a backup running to collide with the next one.
	crate::child_confinement::confine_children();

	metrics::init_metrics();
	metrics::record_activity();

	let pool = daemon_config.pg_pool.clone();

	let canopy_client = match CanopyClient::new(
		daemon_config.device_key_pem.as_ref().map(|r| r.0.as_str()),
		crate::http_builder,
	)
	.await
	{
		Ok(Some(client)) => {
			if client.is_tailscale().await {
				info!("canopy client ready via tailscale");
			} else {
				info!("canopy client ready via mTLS");
			}
			let client = Arc::new(client);
			let renew = client.clone();
			tokio::spawn(async move {
				let mut interval = tokio::time::interval(crate::canopy::CERT_RENEW_AFTER);
				interval.tick().await; // skip the immediate first tick
				loop {
					interval.tick().await;
					if !renew.is_tailscale().await {
						info!("renewing canopy mTLS certificate");
						if let Err(err) = renew.renew().await {
							error!("failed to renew canopy cert: {}", LogError(&err));
						}
					}
				}
			});
			Some(client)
		}
		Ok(None) => {
			info!(
				"no canopy auth path available (no tailscale, no device key); canopy posting will be skipped"
			);
			None
		}
		Err(err) => {
			error!("failed to build canopy client: {}", LogError(&err));
			None
		}
	};

	// Reload channel: the SIGHUP/SIGUSR1 handler (and the `/reload` HTTP control)
	// bump it; tasks watch it to refresh without a restart.
	let (reload_tx, reload_rx) = tokio::sync::watch::channel(0u64);
	let reload_tx = Arc::new(reload_tx);

	let (event_tx, mut event_rx) = mpsc::channel(100);

	let ctx = Arc::new(InternalContext {
		pg_pool: pool,
		http_client: crate::http_client(),
		canopy_client,
		reload: reload_rx,
		restart: Some(RestartTrigger {
			events: event_tx.clone(),
		}),
	});

	// Control handle for the HTTP server's `/reload` and `/restart` endpoints.
	let control = DaemonControl {
		reload: reload_tx.clone(),
		events: event_tx.clone(),
	};

	// Start HTTP server
	if !daemon_config.no_server {
		let ctx_for_server = ctx.clone();
		let background_tasks_for_server = daemon_config.background_tasks.clone();
		let server_addrs = daemon_config.server_addrs.clone();
		let watchdog_timeout = daemon_config.watchdog_timeout;
		let backups = daemon_config.backups.clone();
		let metrics = daemon_config.metrics.clone();
		let binary_version = daemon_config.binary_version.clone();
		tokio::spawn(async move {
			http_server::start_server(
				ctx_for_server,
				server_addrs,
				watchdog_timeout,
				&background_tasks_for_server,
				control,
				backups,
				metrics,
				binary_version,
			)
			.await;
		});
	}

	// SIGINT handler
	let signal_tx = event_tx.clone();
	tokio::spawn(async move {
		match tokio::signal::ctrl_c().await {
			Ok(()) => {
				info!("received SIGINT, shutting down");
				let _ = signal_tx.send(DaemonEvent::Shutdown).await;
			}
			Err(err) => {
				error!("unable to listen for shutdown signal: {}", err);
			}
		}
	});

	// External shutdown signal (for Windows service)
	let external_signal_tx = event_tx.clone();
	tokio::spawn(async move {
		let _ = external_shutdown.await;
		info!("received external shutdown signal");
		let _ = external_signal_tx.send(DaemonEvent::Shutdown).await;
	});

	#[cfg(unix)]
	{
		use tokio::signal::unix::{SignalKind, signal};
		let signal_tx_term = event_tx.clone();
		tokio::spawn(async move {
			let mut sigterm =
				signal(SignalKind::terminate()).expect("failed to setup SIGTERM handler");
			sigterm.recv().await;
			info!("received SIGTERM, shutting down");
			let _ = signal_tx_term.send(DaemonEvent::Shutdown).await;
		});

		// Reload on SIGHUP (sent by the unit's ExecReload) or SIGUSR1:
		// notify systemd we're reloading, bump the reload channel so tasks
		// refresh, then notify ready again. The reload work itself is async and
		// best-effort, so READY is sent once the refresh is dispatched.
		tokio::spawn(async move {
			let mut sighup = signal(SignalKind::hangup()).expect("failed to setup SIGHUP handler");
			let mut sigusr1 =
				signal(SignalKind::user_defined1()).expect("failed to setup SIGUSR1 handler");
			loop {
				tokio::select! {
					_ = sighup.recv() => {}
					_ = sigusr1.recv() => {}
				}
				info!("received reload signal; refreshing");
				let mut reloading = vec![sd_notify::NotifyState::Reloading];
				if let Ok(stamp) = sd_notify::NotifyState::monotonic_usec_now() {
					reloading.push(stamp);
				}
				let _ = sd_notify::notify(&reloading);
				reload_tx.send_modify(|n| *n = n.wrapping_add(1));
				let _ = sd_notify::notify(&[sd_notify::NotifyState::Ready]);
			}
		});
	}
	#[cfg(not(unix))]
	let _ = reload_tx; // no reload signals off Unix; tasks keep their other triggers

	// Registered background tasks (e.g. the doctor sweep). Each ticks at its
	// own interval; errors are logged but don't tear down the daemon.
	for task in &daemon_config.background_tasks {
		let task = task.clone();
		let task_ctx = TaskContext::from_internal(&ctx);
		info!(name = task.name(), interval = ?task.interval(), "registering background task");
		tokio::spawn(async move {
			let mut tick = tokio::time::interval(task.interval());
			tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
			loop {
				tick.tick().await;
				metrics::record_activity();
				if let Err(err) = task.run(&task_ctx).await {
					error!(
						name = task.name(),
						"background task failed: {}",
						LogError(&err)
					);
				}
			}
		});
	}

	// Watchdog: if no task has ticked within the timeout, shut down so the
	// service manager (Windows SCM / systemd / etc.) can restart us.
	if let Some(watchdog_timeout) = daemon_config.watchdog_timeout {
		let watchdog_tx = event_tx.clone();
		tokio::spawn(async move {
			// Give the daemon time to start up and run its first tick
			let grace = watchdog_timeout.max(Duration::from_secs(60));
			tokio::time::sleep(grace).await;

			let mut check_interval = tokio::time::interval(Duration::from_secs(30));
			check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
			loop {
				check_interval.tick().await;
				let last = metrics::last_activity_timestamp();
				let now = jiff::Timestamp::now().as_second();
				let elapsed = Duration::from_secs(now.saturating_sub(last) as u64);
				if elapsed > watchdog_timeout {
					error!(
						?elapsed,
						?watchdog_timeout,
						"watchdog: no task activity detected within timeout, shutting down"
					);
					let _ = watchdog_tx.send(DaemonEvent::WatchdogTimeout).await;
					break;
				}
			}
		});
	}

	info!("daemon started successfully");
	// Tell systemd (Type=notify[-reload]) we're up; no-op when not under systemd.
	// The status line surfaces the running version and the canopy transport
	// (which is fixed at startup), so `systemctl status` shows them at a glance.
	#[cfg(unix)]
	{
		let canopy = match &ctx.canopy_client {
			Some(client) if client.is_tailscale().await => "canopy via tailscale",
			Some(_) => "canopy via mTLS",
			None => "canopy not connected",
		};
		let status = format!(
			"monitoring; bestool {}; {canopy}",
			daemon_config.binary_version
		);
		let _ = sd_notify::notify(&[
			sd_notify::NotifyState::Ready,
			sd_notify::NotifyState::Status(&status),
		]);
	}

	// Block until the first lifecycle event arrives: a shutdown signal, or the
	// watchdog firing. `None` means every sender was dropped, which we treat as
	// a shutdown too.
	let event = event_rx.recv().await;
	#[cfg(unix)]
	let _ = sd_notify::notify(&[sd_notify::NotifyState::Stopping]);
	match event {
		Some(DaemonEvent::Shutdown) | None => {
			info!("daemon stopped");
			Ok(())
		}
		Some(DaemonEvent::Restart) => {
			// Exit non-zero so the service manager restarts us (systemd
			// `Restart=`, Windows SCM recovery).
			info!("restart requested; exiting for the service manager to restart");
			Err(miette!("restart requested"))
		}
		Some(DaemonEvent::WatchdogTimeout) => {
			error!("daemon exiting due to watchdog timeout");
			Err(miette!("watchdog timeout: no task activity detected"))
		}
	}
}
