use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use miette::{IntoDiagnostic, Result, miette};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::{RwLock, mpsc, oneshot};
use tracing::{debug, error, info, warn};

use crate::{
	DaemonConfig, LogError,
	alert::InternalContext,
	canopy::CanopyClient,
	events::{EventContext, EventType},
	http_server, metrics,
	scheduler::Scheduler,
	state_file,
	tasks::TaskContext,
};

enum DaemonEvent {
	FileChanged,
	Shutdown,
	WatchdogTimeout,
	ResolveGlobs,
}

struct WatchManager {
	watcher: notify::RecommendedWatcher,
	watched_paths: HashSet<PathBuf>,
}

impl WatchManager {
	fn new(event_tx: mpsc::Sender<DaemonEvent>) -> Result<Self> {
		let watcher =
			notify::recommended_watcher(move |res: std::result::Result<Event, _>| match res {
				Ok(event) => match event.kind {
					EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
						debug!(?event, "file system event detected");
						let _ = event_tx.blocking_send(DaemonEvent::FileChanged);
					}
					_ => {}
				},
				Err(e) => error!("watch error: {:?}", e),
			})
			.into_diagnostic()?;

		Ok(Self {
			watcher,
			watched_paths: HashSet::new(),
		})
	}

	fn update_watches(&mut self, paths: &[PathBuf]) -> Result<()> {
		let new_paths: HashSet<_> = paths.iter().cloned().collect();

		// Remove watches for paths that no longer exist
		for old_path in &self.watched_paths {
			if !new_paths.contains(old_path) {
				debug!(?old_path, "removing watch for path");
				if let Err(e) = self.watcher.unwatch(old_path) {
					warn!(?old_path, "failed to remove watch: {e}");
				}
			}
		}

		// Add watches for new paths
		for new_path in &new_paths {
			if !self.watched_paths.contains(new_path) && new_path.exists() {
				debug!(?new_path, "adding watch for path");
				if let Err(e) = self.watcher.watch(new_path, RecursiveMode::Recursive) {
					warn!(?new_path, "failed to watch path: {e}");
				}
			}
		}

		self.watched_paths = new_paths;
		Ok(())
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
	run_with_shutdown_and_reload(daemon_config, external_shutdown, None).await
}

pub async fn run_with_shutdown_and_reload(
	daemon_config: DaemonConfig,
	external_shutdown: oneshot::Receiver<()>,
	external_reload: Option<tokio::sync::mpsc::Receiver<()>>,
) -> Result<()> {
	info!("starting alertd daemon");

	// Initialize metrics
	metrics::init_metrics();
	metrics::record_activity();

	debug!(database_url = %daemon_config.database_url, "creating database connection pool");

	let pool =
		bestool_postgres::pool::create_pool(&daemon_config.database_url, "bestool-alertd").await?;

	let canopy_client = match CanopyClient::new(
		daemon_config.tamanu_version.clone(),
		daemon_config.device_key_pem.as_ref().map(|r| r.0.as_str()),
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
				"no canopy auth path available (no tailscale, no device key); canopy targets will be skipped"
			);
			None
		}
		Err(err) => {
			error!("failed to build canopy client: {}", LogError(&err));
			None
		}
	};

	let ctx = Arc::new(InternalContext {
		pg_pool: pool,
		http_client: reqwest::Client::new(),
		canopy_client,
	});

	let scheduler = Arc::new(Scheduler::new(
		daemon_config.alert_globs.clone(),
		ctx.clone(),
		daemon_config.email.clone(),
		daemon_config.dry_run,
	));

	// Resolve the persistence file path and seed cold-start state from it.
	// On dry-run we skip persistence entirely — the daemon doesn't tick.
	let state_file_path = (!daemon_config.dry_run)
		.then(state_file::default_state_file_path)
		.flatten();
	if let Some(path) = state_file_path.as_ref() {
		info!(?path, "alertd state file");
		let persisted = state_file::read(path);
		scheduler.set_pending_hydration(persisted).await;
	} else if !daemon_config.dry_run {
		warn!("could not resolve a state directory; running without persistence");
	}

	scheduler.load_and_schedule_alerts().await?;

	// If dry run, execute all alerts once and quit
	if daemon_config.dry_run {
		info!("dry run mode: executing all alerts once");
		scheduler.execute_all_alerts_once().await?;
		info!("dry run complete");
		return Ok(());
	}

	let (event_tx, mut event_rx) = mpsc::channel(100);
	let (reload_tx, mut reload_rx) = mpsc::channel::<()>(10);

	// Start HTTP server
	if !daemon_config.no_server {
		let event_manager_for_server = scheduler.get_event_manager();
		let ctx_for_server = ctx.clone();
		let email_for_server = daemon_config.email.clone();
		let dry_run_for_server = daemon_config.dry_run;
		let scheduler_for_server = scheduler.clone();
		let reload_tx_for_server = reload_tx.clone();
		tokio::spawn(async move {
			// Wait for event manager to be initialised
			let event_mgr = loop {
				let guard = event_manager_for_server.read().await;
				if let Some(ref mgr) = *guard {
					break Some(Arc::new(mgr.clone()));
				}
				drop(guard);
				tokio::time::sleep(std::time::Duration::from_millis(100)).await;
			};
			let watchdog_timeout_for_server = daemon_config.watchdog_timeout;
			http_server::start_server(
				reload_tx_for_server,
				event_mgr,
				ctx_for_server,
				email_for_server,
				dry_run_for_server,
				daemon_config.server_addrs.clone(),
				scheduler_for_server,
				watchdog_timeout_for_server,
			)
			.await;
		});
	}

	// Set up file watcher
	let watch_manager = Arc::new(RwLock::new(WatchManager::new(event_tx.clone())?));

	// Get initial paths to watch
	let initial_paths = scheduler.get_resolved_paths().await;
	watch_manager.write().await.update_watches(&initial_paths)?;
	info!(count = initial_paths.len(), "watching paths for changes");

	// Setup signal handler
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

		let scheduler_hup = scheduler.clone();
		let watch_manager_hup = watch_manager.clone();
		let ctx_hup = ctx.clone();
		tokio::spawn(async move {
			let mut sighup = signal(SignalKind::hangup()).expect("failed to setup SIGHUP handler");
			loop {
				sighup.recv().await;
				info!("received SIGHUP, reloading configuration");
				metrics::inc_reloads();
				refresh_canopy_client(&ctx_hup).await;
				if let Err(err) = scheduler_hup.reload_alerts().await {
					error!("failed to reload alerts: {}", LogError(&err));
				} else {
					// Update watches after reload
					let new_paths = scheduler_hup.get_resolved_paths().await;
					if let Err(err) = watch_manager_hup.write().await.update_watches(&new_paths) {
						error!("failed to update watches: {}", LogError(&err));
					}
				}
			}
		});
	}

	// Periodically re-resolve globs (every 5 minutes)
	let glob_resolve_tx = event_tx.clone();
	tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(5 * 60));
		interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
		loop {
			interval.tick().await;
			debug!("triggering periodic glob resolution");
			let _ = glob_resolve_tx.send(DaemonEvent::ResolveGlobs).await;
		}
	});

	// Persistence task: wake on state_dirty, debounce, write the file.
	if let Some(path) = state_file_path.clone() {
		let dirty = scheduler.state_dirty();
		let snap_scheduler = scheduler.clone();
		tokio::spawn(async move {
			loop {
				dirty.notified().await;
				// Coalesce bursts: a single tick that touches several fields,
				// or several alerts changing within a window, all collapse
				// into one write.
				tokio::time::sleep(Duration::from_millis(500)).await;
				let snapshot = snap_scheduler.snapshot_for_persistence().await;
				match state_file::write(&path, &snapshot) {
					Ok(()) => debug!(?path, "wrote alertd state file"),
					Err(err) => error!(?path, "failed to write state file: {}", LogError(&err)),
				}
			}
		});
	}

	// Periodic database health check (every 30 seconds)
	{
		let health_ctx = ctx.clone();
		let health_scheduler = scheduler.clone();
		let health_email = daemon_config.email.clone();
		let health_dry_run = daemon_config.dry_run;
		let health_db_url = daemon_config.database_url.clone();
		tokio::spawn(async move {
			// Seed from persisted state so a recovery that happens while the
			// daemon was down still produces a canopy clear on the next tick.
			let mut was_down = health_scheduler.database_was_down().await;
			let mut check_interval = tokio::time::interval(Duration::from_secs(30));
			check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
			loop {
				check_interval.tick().await;

				let healthy = match health_ctx.pg_pool.get_timeout(Duration::from_secs(5)).await {
					Ok(conn) => conn.simple_query("SELECT 1").await.is_ok(),
					Err(_) => false,
				};

				if healthy {
					if was_down {
						info!("database connection restored, clearing database-down event");
						was_down = false;
						health_scheduler.set_database_was_down(false).await;
						if let Some(ref event_mgr) =
							*health_scheduler.get_event_manager().read().await
							&& let Err(err) = event_mgr
								.trigger_clear(
									EventType::DatabaseDown,
									&health_ctx,
									health_dry_run,
									None,
								)
								.await
						{
							error!("failed to clear database-down event: {}", LogError(&err));
						}
					}
				} else if !was_down {
					was_down = true;
					health_scheduler.set_database_was_down(true).await;
					error!("database health check failed, triggering database-down event");

					// Redact password from URL for the alert context
					let redacted_url = match url::Url::parse(&health_db_url) {
						Ok(mut parsed) => {
							if parsed.password().is_some() {
								let _ = parsed.set_password(Some("***"));
							}
							parsed.to_string()
						}
						Err(_) => "(unparsable)".to_string(),
					};

					let event_context = EventContext::DatabaseDown {
						database_url: redacted_url,
						error_message: "health check SELECT 1 failed or timed out".to_string(),
					};

					if let Some(ref event_mgr) = *health_scheduler.get_event_manager().read().await
					{
						if let Err(err) = event_mgr
							.trigger_event(
								EventType::DatabaseDown,
								&health_ctx,
								health_email.as_ref(),
								health_dry_run,
								event_context,
								None,
							)
							.await
						{
							error!("failed to trigger database-down event: {}", LogError(&err));
						}
					} else {
						warn!(
							"event manager not yet initialized, cannot trigger database-down event"
						);
					}
				} else {
					debug!("database still unreachable");
				}
			}
		});
	}

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

	// Watchdog: if no alert task has ticked within the timeout, shut down so the
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
						"watchdog: no alert activity detected within timeout, shutting down"
					);
					let _ = watchdog_tx.send(DaemonEvent::WatchdogTimeout).await;
					break;
				}
			}
		});
	}

	// Listen for external reload signals (e.g., from Windows TIME_CHANGE event)
	if let Some(mut external_reload_rx) = external_reload {
		let reload_tx = reload_tx.clone();
		tokio::spawn(async move {
			while (external_reload_rx.recv().await).is_some() {
				info!("received external reload signal");
				let _ = reload_tx.send(()).await;
			}
		});
	}

	let mut reload_debounce = tokio::time::interval(Duration::from_secs(2));
	reload_debounce.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
	let mut needs_reload = false;

	info!("daemon started successfully");

	loop {
		tokio::select! {
			Some(event) = event_rx.recv() => {
				match event {
					DaemonEvent::FileChanged => {
						needs_reload = true;
					}
					DaemonEvent::ResolveGlobs => {
						debug!("re-resolving glob patterns");
						if let Err(err) = scheduler.check_and_reload_if_paths_changed().await {
							error!("failed to check and reload: {}", LogError(&err));
						} else {
							// Update watches with new paths
							let new_paths = scheduler.get_resolved_paths().await;
							if let Err(err) = watch_manager.write().await.update_watches(&new_paths) {
								error!("failed to update watches: {}", LogError(&err));
							}
						}
					}
					DaemonEvent::Shutdown => {
							scheduler.shutdown().await;
							info!("daemon stopped");
							break;
						}
						DaemonEvent::WatchdogTimeout => {
							scheduler.shutdown().await;
							error!("daemon exiting due to watchdog timeout");
							return Err(miette!("watchdog timeout: no alert activity detected"));
						}
				}
			}
			Some(()) = reload_rx.recv() => {
				info!("reloading alerts via HTTP");
				metrics::inc_reloads();
				refresh_canopy_client(&ctx).await;
				if let Err(err) = scheduler.reload_alerts().await {
					error!("failed to reload alerts: {}", LogError(&err));
				} else {
					// Update watches after reload
					let new_paths = scheduler.get_resolved_paths().await;
					if let Err(err) = watch_manager.write().await.update_watches(&new_paths) {
						error!("failed to update watches: {}", LogError(&err));
					}
				}
			}
			_ = reload_debounce.tick() => {
				if needs_reload {
					needs_reload = false;
					info!("reloading alerts due to file system changes");
					metrics::inc_reloads();
					refresh_canopy_client(&ctx).await;
					if let Err(err) = scheduler.reload_alerts().await {
						error!("failed to reload alerts: {}", LogError(&err));
					} else {
						// Update watches after reload
						let new_paths = scheduler.get_resolved_paths().await;
						if let Err(err) = watch_manager.write().await.update_watches(&new_paths) {
							error!("failed to update watches: {}", LogError(&err));
						}
					}
				}
			}
		}
	}

	Ok(())
}

/// Re-probe canopy auth on reload; logs failures but never blocks the reload.
async fn refresh_canopy_client(ctx: &InternalContext) {
	let Some(client) = ctx.canopy_client.as_ref() else {
		return;
	};
	if let Err(err) = client.refresh().await {
		error!("canopy client refresh failed: {}", LogError(&err));
	}
}
