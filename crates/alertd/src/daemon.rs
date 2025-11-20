use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use miette::{IntoDiagnostic, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};

use crate::{DaemonConfig, alert::InternalContext, scheduler::Scheduler};

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
	info!("starting alertd daemon");

	debug!(database_url = %daemon_config.database_url, "connecting to database");

	let pg_config = daemon_config
		.database_url
		.parse::<tokio_postgres::Config>()
		.into_diagnostic()?;
	let (client, connection) = pg_config
		.connect(tokio_postgres::NoTls)
		.await
		.into_diagnostic()?;

	tokio::spawn(async move {
		if let Err(e) = connection.await {
			error!("database connection error: {}", e);
		}
	});

	let ctx = Arc::new(InternalContext { pg_client: client });

	let default_interval = Duration::from_secs(60 * 15); // 15 minutes default

	let scheduler = Arc::new(Scheduler::new(
		daemon_config.alert_globs.clone(),
		default_interval,
		ctx,
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

		let _signal_tx_hup = event_tx.clone();
		let scheduler_hup = scheduler.clone();
		let watch_manager_hup = watch_manager.clone();
		tokio::spawn(async move {
			let mut sighup = signal(SignalKind::hangup()).expect("failed to setup SIGHUP handler");
			loop {
				sighup.recv().await;
				info!("received SIGHUP, reloading configuration");
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
			_ = reload_debounce.tick() => {
				if needs_reload {
					needs_reload = false;
					info!("reloading alerts due to file system changes");
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
