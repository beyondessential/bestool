use std::{sync::Arc, time::Duration};

use miette::{IntoDiagnostic, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::{DaemonConfig, alert::InternalContext, scheduler::Scheduler};

enum DaemonEvent {
	FileChanged,
	Shutdown,
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
		daemon_config.alert_dirs.clone(),
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

	let watcher_tx = event_tx.clone();
	let alert_dirs = daemon_config.alert_dirs.clone();
	let _watcher = tokio::task::spawn_blocking(move || {
		let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| match res {
			Ok(event) => match event.kind {
				EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
					debug!(?event, "file system event detected");
					let _ = watcher_tx.blocking_send(DaemonEvent::FileChanged);
				}
				_ => {}
			},
			Err(e) => error!("watch error: {:?}", e),
		})
		.expect("failed to create file watcher");

		for dir in &alert_dirs {
			if dir.exists() {
				if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
					warn!(?dir, "failed to watch directory: {e}");
				} else {
					info!(?dir, "watching directory for changes");
				}
			}
		}

		// Keep the watcher alive
		std::thread::park();
	});

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
		tokio::spawn(async move {
			let mut sighup = signal(SignalKind::hangup()).expect("failed to setup SIGHUP handler");
			loop {
				sighup.recv().await;
				info!("received SIGHUP, reloading configuration");
				if let Err(err) = scheduler_hup.reload_alerts().await {
					error!("failed to reload alerts: {err:?}");
				}
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
					}
				}
			}
		}
	}

	Ok(())
}
