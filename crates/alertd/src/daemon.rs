use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use miette::{IntoDiagnostic, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::{RwLock, mpsc, oneshot};
use tracing::{debug, error, info, warn};

use crate::{DaemonConfig, alert::InternalContext, http_server, metrics, scheduler::Scheduler};

enum DaemonEvent {
	FileChanged,
	Shutdown,
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
	info!("starting alertd daemon");

	// Initialize metrics
	metrics::init_metrics();

	debug!(database_url = %daemon_config.database_url, "creating database connection pool");

	let pool =
		bestool_postgres::pool::create_pool(&daemon_config.database_url, "bestool-alertd").await?;

	let ctx = Arc::new(InternalContext { pg_pool: pool });

	let scheduler = Arc::new(Scheduler::new(
		daemon_config.alert_globs.clone(),
		ctx.clone(),
		daemon_config.email.clone(),
		daemon_config.dry_run,
	));

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
			http_server::start_server(
				reload_tx.clone(),
				event_mgr,
				ctx_for_server,
				email_for_server,
				dry_run_for_server,
				daemon_config.server_addrs.clone(),
				scheduler_for_server,
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
		tokio::spawn(async move {
			let mut sighup = signal(SignalKind::hangup()).expect("failed to setup SIGHUP handler");
			loop {
				sighup.recv().await;
				info!("received SIGHUP, reloading configuration");
				metrics::inc_reloads();
				if let Err(err) = scheduler_hup.reload_alerts().await {
					error!("failed to reload alerts: {err:?}");
				} else {
					// Update watches after reload
					let new_paths = scheduler_hup.get_resolved_paths().await;
					if let Err(err) = watch_manager_hup.write().await.update_watches(&new_paths) {
						error!("failed to update watches: {err:?}");
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
							error!("failed to check and reload: {err:?}");
						} else {
							// Update watches with new paths
							let new_paths = scheduler.get_resolved_paths().await;
							if let Err(err) = watch_manager.write().await.update_watches(&new_paths) {
								error!("failed to update watches: {err:?}");
							}
						}
					}
					DaemonEvent::Shutdown => {
						scheduler.shutdown().await;
						info!("daemon stopped");
						break;
					}
				}
			}
			Some(()) = reload_rx.recv() => {
				info!("reloading alerts via HTTP");
				metrics::inc_reloads();
				if let Err(err) = scheduler.reload_alerts().await {
					error!("failed to reload alerts: {err:?}");
				} else {
					// Update watches after reload
					let new_paths = scheduler.get_resolved_paths().await;
					if let Err(err) = watch_manager.write().await.update_watches(&new_paths) {
						error!("failed to update watches: {err:?}");
					}
				}
			}
			_ = reload_debounce.tick() => {
				if needs_reload {
					needs_reload = false;
					info!("reloading alerts due to file system changes");
					metrics::inc_reloads();
					if let Err(err) = scheduler.reload_alerts().await {
						error!("failed to reload alerts: {err:?}");
					} else {
						// Update watches after reload
						let new_paths = scheduler.get_resolved_paths().await;
						if let Err(err) = watch_manager.write().await.update_watches(&new_paths) {
							error!("failed to update watches: {err:?}");
						}
					}
				}
			}
		}
	}

	Ok(())
}
