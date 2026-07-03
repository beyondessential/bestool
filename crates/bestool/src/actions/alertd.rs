use std::{net::SocketAddr, sync::Arc};

use clap::{Parser, Subcommand};
use miette::Result;
use tracing::warn;

use bestool_alertd::doctor::DoctorTask;

use crate::actions::Context;

/// Run the healthcheck daemon
///
/// Periodically runs the doctor healthcheck sweep and posts the result to
/// canopy. On a Tamanu host, database and device-key configuration is read from
/// Tamanu's config files; on other hosts the daemon still runs and posts
/// sweeps, with every Tamanu-dependent check skipped.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct AlertdArgs {
	#[command(subcommand)]
	command: Command,
}

/// Common arguments for running the daemon
#[derive(Debug, Clone, Parser)]
struct DaemonArgs {
	/// Deprecated, does nothing.
	///
	/// Previously selected the alert definition files to load. The daemon no
	/// longer loads alert definitions; the option is still accepted so
	/// existing invocations keep working until they are migrated.
	#[arg(long, value_name = "GLOB")]
	glob: Vec<String>,

	/// Disable the HTTP server
	#[arg(long)]
	no_server: bool,

	/// HTTP server bind address(es)
	///
	/// Can be provided multiple times. The server will attempt to bind to each address
	/// in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
	#[arg(long)]
	server_addr: Vec<SocketAddr>,

	/// Watchdog timeout in seconds
	///
	/// If no task reports activity within this many seconds, the daemon
	/// will exit so the service manager can restart it. Defaults to 600 (10 minutes).
	#[arg(long, default_value = "600")]
	watchdog_timeout: u64,

	/// Disable the watchdog
	///
	/// By default, the daemon will exit if no task activity is detected within
	/// the watchdog timeout. This flag disables that behaviour.
	#[arg(long)]
	no_watchdog: bool,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
	/// Run the healthcheck daemon
	///
	/// Starts the daemon which runs the doctor healthcheck sweep on a schedule
	/// and posts the result to canopy.
	Run {
		#[command(flatten)]
		daemon: DaemonArgs,
	},

	/// Show status and health of a running daemon
	///
	/// Connects to the running daemon's HTTP API and displays version, uptime,
	/// health, and watchdog information. Exits with code 1 if the daemon is unhealthy.
	Status {
		/// HTTP server address(es) to try
		///
		/// Can be provided multiple times. Will attempt to connect to each address
		/// in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
		#[arg(long)]
		server_addr: Vec<SocketAddr>,
	},

	/// Reload a running daemon
	///
	/// Asks the daemon to re-register backup capabilities and pick up changes
	/// under /etc/bestool/backups, without restarting.
	Reload {
		/// HTTP server address(es) to try (defaults to [::1]:8271 and 127.0.0.1:8271)
		#[arg(long)]
		server_addr: Vec<SocketAddr>,
	},

	/// Restart a running daemon
	///
	/// Asks the daemon to exit so the service manager restarts it — e.g. to pick
	/// up a freshly-installed bestool binary.
	Restart {
		/// HTTP server address(es) to try (defaults to [::1]:8271 and 127.0.0.1:8271)
		#[arg(long)]
		server_addr: Vec<SocketAddr>,
	},

	/// Install the daemon as a Windows service
	///
	/// Creates a Windows service named 'bestool-alertd' that will start automatically
	/// and starts it immediately.
	#[cfg(windows)]
	Install,

	/// Uninstall the Windows service
	///
	/// Stops the 'bestool-alertd' Windows service if running and then removes it.
	#[cfg(windows)]
	Uninstall,

	/// Configure failure recovery on an existing Windows service
	///
	/// Updates the 'bestool-alertd' service to automatically restart on failure.
	/// This is done automatically on new installs, but can be run separately to
	/// update an already-installed service.
	#[cfg(windows)]
	ConfigureRecovery,

	#[cfg(windows)]
	#[command(hide = true)]
	Service {
		#[command(flatten)]
		daemon: DaemonArgs,
	},
}

pub async fn run(args: AlertdArgs, ctx: Context) -> Result<()> {
	match args.command {
		Command::Status { server_addr } => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::get_status(&addrs).await
		}
		Command::Reload { server_addr } => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::reload(&addrs).await
		}
		Command::Restart { server_addr } => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::restart(&addrs).await
		}
		Command::Run { daemon } => {
			let daemon_config = build_config(&ctx, daemon).await?;
			bestool_alertd::run(daemon_config).await
		}
		#[cfg(windows)]
		Command::Install => {
			use std::ffi::OsString;
			bestool_alertd::windows_service::install_service_with_args(&[
				OsString::from("alertd"),
				OsString::from("service"),
			])
		}
		#[cfg(windows)]
		Command::Uninstall => bestool_alertd::windows_service::uninstall_service(),
		#[cfg(windows)]
		Command::ConfigureRecovery => bestool_alertd::windows_service::configure_recovery(),
		#[cfg(windows)]
		Command::Service { daemon } => {
			// Check and auto-apply recovery configuration if needed
			match bestool_alertd::windows_service::is_recovery_configured() {
				Ok(false) => {
					tracing::info!("failure recovery not configured, applying automatically");
					if let Err(e) = bestool_alertd::windows_service::configure_recovery() {
						tracing::warn!("failed to auto-configure recovery: {e}");
					}
				}
				Err(e) => {
					tracing::warn!("failed to check recovery configuration: {e}");
				}
				Ok(true) => {}
			}

			let daemon_config = build_config(&ctx, daemon).await?;
			bestool_alertd::windows_service::run_service(daemon_config)
		}
	}
}

/// Build the doctor task, wiring the canopy backup trigger when backups are
/// compiled in.
fn doctor_task(
	version: String,
	tamanu: Option<bestool_alertd::doctor::SweepTamanu>,
) -> DoctorTask {
	DoctorTask::new(version, tamanu)
}

/// Register the daemon's tasks. When backups are compiled in, this creates the
/// backup registry once and shares it: the doctor task's canopy-trigger dispatch
/// and the `backup` task (its `run`/`running` endpoints) drive the same runs.
fn with_daemon_tasks(
	config: bestool_alertd::DaemonConfig,
	doctor: DoctorTask,
) -> bestool_alertd::DaemonConfig {
	#[cfg(feature = "canopy-backup")]
	let (config, doctor) = {
		let registry = bestool_alertd::BackupRegistry::new(backup_runner());
		let doctor = doctor.with_backup_dispatch(backup_dispatch(registry.clone()));
		let config = config
			.with_backups(registry.clone())
			.with_task(Arc::new(bestool_alertd::BackupTask::new(registry.clone())))
			.with_task(Arc::new(backup::BackupCapabilitiesTask::new(registry)));
		(config, doctor)
	};

	// On Windows the daemon keeps bestool current itself; on Linux the package
	// manager owns updates.
	#[cfg(all(windows, feature = "self-update"))]
	let config = config.with_task(Arc::new(
		crate::actions::self_update::task::SelfUpdateTask::new(),
	));

	config.with_task(Arc::new(doctor))
}

/// The in-process backup trigger: routes each type canopy requests via
/// `backup_now` through the registry (which starts a run, or no-ops onto the one
/// already in flight), draining the status stream — the run reports to canopy
/// itself.
#[cfg(feature = "canopy-backup")]
fn backup_dispatch(
	registry: Arc<bestool_alertd::BackupRegistry>,
) -> bestool_alertd::doctor::BackupDispatch {
	use futures::StreamExt as _;

	Arc::new(move |types: Vec<String>| {
		for backup_type in types {
			let registry = registry.clone();
			tokio::spawn(async move {
				let mut stream = registry.ensure_run(backup_type).await;
				while stream.next().await.is_some() {}
			});
		}
	})
}

/// Builds the registry's runner: drives [`run_backup`] with a sink, mapping its
/// [`BackupEvent`]s to JSON status lines and guaranteeing a terminal event.
///
/// [`run_backup`]: crate::actions::canopy::backup::run_backup
/// [`BackupEvent`]: crate::actions::canopy::backup::BackupEvent
#[cfg(feature = "canopy-backup")]
fn backup_runner() -> bestool_alertd::BackupRunner {
	use serde_json::{Value, json};
	use tokio::sync::mpsc;

	use crate::actions::canopy::backup::{BackupEvent, run_backup};

	fn to_json(event: BackupEvent) -> Value {
		match event {
			BackupEvent::Started { run_id } => json!({"event": "started", "runId": run_id}),
			BackupEvent::Phase(phase) => json!({"event": "phase", "phase": phase}),
			BackupEvent::Progress(status) => json!({"event": "progress", "status": status}),
			BackupEvent::Done {
				snapshot_id,
				bytes_uploaded,
			} => json!({
				"event": "done", "success": true,
				"snapshotId": snapshot_id, "uploadedBytes": bytes_uploaded,
			}),
			BackupEvent::Failed { error } => {
				json!({"event": "error", "success": false, "message": error})
			}
		}
	}

	Arc::new(
		move |backup_type: String,
		      out: mpsc::UnboundedSender<Value>|
		      -> futures::future::BoxFuture<'static, ()> {
			Box::pin(async move {
				let (tx, mut rx) = mpsc::unbounded_channel::<BackupEvent>();
				let run =
					tokio::spawn(async move { run_backup(&backup_type, None, None, Some(tx)).await });

				let mut terminal = false;
				while let Some(event) = rx.recv().await {
					terminal |= matches!(event, BackupEvent::Done { .. } | BackupEvent::Failed { .. });
					let _ = out.send(to_json(event));
				}

				// run_backup has returned (its sink dropped). Guarantee a
				// terminal event for any early-exit path that emitted none.
				if !terminal {
					let event = match run.await {
						Ok(Ok(())) => json!({"event": "done", "success": true}),
						Ok(Err(err)) => {
							json!({"event": "error", "success": false, "message": format!("{err}")})
						}
						Err(join) => json!({
							"event": "error", "success": false,
							"message": format!("backup task aborted: {join}"),
						}),
					};
					let _ = out.send(event);
				}
			})
		},
	)
}

/// A background task that registers this server's backup capabilities with
/// canopy (the types of every def in the backups directory).
///
/// It's a resident task: after the initial registration it stays running and
/// re-registers when the backups directory changes, when a reload signal
/// (SIGHUP/SIGUSR1) arrives, or on a periodic safety-net tick — so dropping a
/// new def in `/etc/bestool/backups` is picked up without restarting the daemon.
#[cfg(feature = "canopy-backup")]
mod backup {
	use std::{sync::Arc, time::Duration};

	use futures::future::BoxFuture;
	use miette::Result;
	use tokio::sync::mpsc;
	use tracing::{info, warn};

	use crate::actions::canopy::backup::config;

	/// Re-register at least this often even without an external trigger, so a
	/// missed event still converges.
	const REREGISTER_INTERVAL: Duration = Duration::from_secs(3600);

	pub(super) struct BackupCapabilitiesTask {
		registry: Arc<bestool_alertd::BackupRegistry>,
	}

	impl BackupCapabilitiesTask {
		pub(super) fn new(registry: Arc<bestool_alertd::BackupRegistry>) -> Self {
			Self { registry }
		}
	}

	/// Load the configured backup types, record them for the daemon's status, and
	/// register them with canopy.
	///
	/// The configured types are recorded even when there's no canopy client or no
	/// defs, so the status reflects the host's config regardless of connectivity.
	async fn register(
		registry: &bestool_alertd::BackupRegistry,
		ctx: &bestool_alertd::TaskContext,
	) -> Result<()> {
		let defs = config::load_dir(&config::backups_dir()).await?;
		let types: Vec<String> = defs.into_iter().map(|d| d.r#type).collect();
		registry.set_configured(types.clone()).await;

		let Some(client) = ctx.canopy_client.as_ref() else {
			return Ok(());
		};
		if types.is_empty() {
			return Ok(());
		}
		client.backup_capabilities(&types).await?;
		info!(?types, "registered backup capabilities with canopy");
		Ok(())
	}

	/// Re-register, logging the trigger; failures are warned, never fatal.
	async fn reregister(
		reason: &str,
		registry: &bestool_alertd::BackupRegistry,
		ctx: &bestool_alertd::TaskContext,
	) {
		info!(reason, "registering backup capabilities");
		if let Err(err) = register(registry, ctx).await {
			warn!("registering backup capabilities failed (will retry): {err}");
		}
	}

	/// Whether an fs event means a backup def was actually added, edited, renamed,
	/// or removed — as opposed to read-noise.
	///
	/// Re-registration reads the backups dir (it opens, reads, and closes every
	/// `*.toml`). The inotify backend watches `OPEN`/`CLOSE_NOWRITE`/`ATTRIB`, so
	/// those reads surface as `Access(_)` events and atime bumps as
	/// `Modify(Metadata)`. Forwarding them makes each re-registration trigger the
	/// next, busy-looping several times a second. Every genuine change also
	/// surfaces as a `Create`/`Remove`/`Modify(Data|Name)` (or, on Windows,
	/// `Modify(Any)`) event, so dropping the access/metadata noise loses nothing.
	///
	/// TODO: when notify 9.0.0 lands, use EventKindMask to filter at the source.
	fn is_real_change(event: &notify::Event) -> bool {
		use notify::{EventKind, event::ModifyKind};

		!matches!(
			event.kind,
			EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_))
		)
	}

	/// Watch the backups directory, sending `()` on any change. Returns the
	/// watcher (kept alive by the caller) or `None` if it couldn't be set up
	/// (e.g. the directory doesn't exist yet) — the periodic tick still covers it.
	fn watch_backups_dir(tx: mpsc::UnboundedSender<()>) -> Option<notify::RecommendedWatcher> {
		use notify::{RecursiveMode, Watcher as _};

		let dir = config::backups_dir();
		let mut watcher =
			notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
				if let Ok(event) = res
					&& is_real_change(&event)
				{
					let _ = tx.send(());
				}
			})
			.inspect_err(|err| warn!("could not create backups-dir watcher: {err}"))
			.ok()?;
		match watcher.watch(&dir, RecursiveMode::NonRecursive) {
			Ok(()) => Some(watcher),
			Err(err) => {
				warn!(dir = %dir.display(), "could not watch backups dir (using timer/signals): {err}");
				None
			}
		}
	}

	/// Coalesce a burst of fs events into one re-registration.
	fn drain(rx: &mut mpsc::UnboundedReceiver<()>) {
		while rx.try_recv().is_ok() {}
	}

	/// Re-register whenever the backups dir changes, a reload signal arrives, or
	/// the safety-net tick fires, calling `on_trigger(reason)` each time.
	///
	/// The fs arm matches `Some(())` deliberately: if the watcher couldn't be set
	/// up its sender is dropped, so `recv()` resolves to `None` immediately and
	/// forever. Matching `Some` leaves that arm unmatched (disabled) instead of
	/// firing in a tight loop — the periodic tick and reload signal still cover
	/// re-registration. Returns when the reload sender is dropped (shutdown).
	async fn event_loop<F, Fut>(
		mut fs_rx: mpsc::UnboundedReceiver<()>,
		mut reload: tokio::sync::watch::Receiver<u64>,
		interval: Duration,
		mut on_trigger: F,
	) where
		F: FnMut(&'static str) -> Fut,
		Fut: std::future::Future<Output = ()>,
	{
		let mut periodic = tokio::time::interval(interval);
		periodic.tick().await; // consume the immediate first tick

		loop {
			tokio::select! {
				_ = periodic.tick() => on_trigger("periodic").await,
				Some(()) = fs_rx.recv() => {
					drain(&mut fs_rx);
					on_trigger("backups dir changed").await;
				}
				changed = reload.changed() => match changed {
					Ok(()) => on_trigger("reload signal").await,
					// Sender dropped → daemon is shutting down.
					Err(_) => break,
				},
			}
		}
	}

	impl bestool_alertd::BackgroundTask for BackupCapabilitiesTask {
		fn name(&self) -> &'static str {
			"backup-capabilities"
		}

		fn interval(&self) -> Duration {
			// run() is resident, so this only gates the (single) first tick.
			REREGISTER_INTERVAL
		}

		fn run<'a>(&'a self, ctx: &'a bestool_alertd::TaskContext) -> BoxFuture<'a, Result<()>> {
			Box::pin(async move {
				reregister("startup", &self.registry, ctx).await;

				let (tx, rx) = mpsc::unbounded_channel();
				// Held for the task's lifetime so events keep arriving. `None` when
				// the dir can't be watched yet; the periodic tick still covers it.
				let _watcher = watch_backups_dir(tx);

				// The daemon turns SIGHUP/SIGUSR1 (and systemd's reload) into a
				// bump on the reload channel; we also re-register on a backups-dir
				// change and a periodic safety-net tick.
				event_loop(rx, ctx.reload.clone(), REREGISTER_INTERVAL, |reason| {
					reregister(reason, &self.registry, ctx)
				})
				.await;
				Ok(())
			})
		}
	}

	#[cfg(test)]
	mod tests {
		use std::sync::{
			Arc,
			atomic::{AtomicUsize, Ordering},
		};

		use super::*;

		/// The reads that re-registration performs surface as `Access`/metadata
		/// events on the watched dir; forwarding them would feed back into another
		/// re-registration, busy-looping. Those must be ignored while genuine
		/// add/edit/remove events still pass.
		#[test]
		fn read_noise_is_not_a_real_change() {
			use notify::{
				Event, EventKind,
				event::{AccessKind, AccessMode, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind},
			};

			let noise = [
				EventKind::Access(AccessKind::Open(AccessMode::Any)),
				EventKind::Access(AccessKind::Close(AccessMode::Read)),
				EventKind::Access(AccessKind::Read),
				EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
			];
			for kind in noise {
				assert!(
					!is_real_change(&Event::new(kind.clone())),
					"{kind:?} should be ignored as read-noise"
				);
			}

			let real = [
				EventKind::Create(CreateKind::File),
				EventKind::Remove(RemoveKind::File),
				EventKind::Modify(ModifyKind::Data(DataChange::Any)),
				EventKind::Modify(ModifyKind::Any),
			];
			for kind in real {
				assert!(
					is_real_change(&Event::new(kind.clone())),
					"{kind:?} should trigger re-registration"
				);
			}
		}

		/// When the watcher can't be set up its sender is dropped, closing the fs
		/// channel. The loop must stay idle (only periodic/reload trigger it), not
		/// busy-loop on the closed channel's instant `None`.
		#[tokio::test]
		async fn closed_fs_channel_does_not_storm() {
			let (fs_tx, fs_rx) = mpsc::unbounded_channel::<()>();
			drop(fs_tx); // simulate watch_backups_dir returning None
			let (reload_tx, reload_rx) = tokio::sync::watch::channel(0u64);

			let count = Arc::new(AtomicUsize::new(0));
			let triggers = count.clone();
			let loop_fut = event_loop(fs_rx, reload_rx, Duration::from_secs(3600), move |_| {
				let triggers = triggers.clone();
				async move {
					triggers.fetch_add(1, Ordering::SeqCst);
				}
			});

			// Run briefly: a busy loop would rack up thousands of triggers.
			let _ = tokio::time::timeout(Duration::from_millis(200), loop_fut).await;
			drop(reload_tx);

			assert_eq!(
				count.load(Ordering::SeqCst),
				0,
				"a closed fs channel must not trigger re-registration"
			);
		}

		/// A reload bump fires exactly one re-registration, and dropping the reload
		/// sender ends the loop.
		#[tokio::test]
		async fn reload_signal_triggers_once_then_shutdown_ends_loop() {
			let (_fs_tx, fs_rx) = mpsc::unbounded_channel::<()>();
			let (reload_tx, reload_rx) = tokio::sync::watch::channel(0u64);

			let count = Arc::new(AtomicUsize::new(0));
			let triggers = count.clone();
			let handle = tokio::spawn(event_loop(
				fs_rx,
				reload_rx,
				Duration::from_secs(3600),
				move |_| {
					let triggers = triggers.clone();
					async move {
						triggers.fetch_add(1, Ordering::SeqCst);
					}
				},
			));

			reload_tx.send_modify(|n| *n = n.wrapping_add(1));
			tokio::time::sleep(Duration::from_millis(50)).await;
			drop(reload_tx); // shutdown

			handle.await.expect("loop task panicked");
			assert_eq!(count.load(Ordering::SeqCst), 1);
		}
	}
}

/// Build the daemon config, reading Tamanu's config files and DB for the
/// database URL and (migrated) device key. Honours a `--root` when invoked via
/// the `tamanu alert` alias; top-level `bestool alertd` probes the default
/// locations. A host with no Tamanu still runs the daemon, with every
/// Tamanu-dependent check skipped.
#[cfg(feature = "alertd-tamanu")]
async fn build_config(ctx: &Context, daemon: DaemonArgs) -> Result<bestool_alertd::DaemonConfig> {
	use std::path::PathBuf;

	use node_semver::Version;
	use tracing::debug;

	use bestool_alertd::doctor::resolve_sweep_tamanu;
	use bestool_tamanu::server_info::{fetch_device_key_with, query_device_key_row};

	let DaemonArgs {
		glob,
		no_server,
		server_addr,
		watchdog_timeout,
		no_watchdog,
	} = daemon;
	if !glob.is_empty() {
		warn!("--glob is deprecated and does nothing; alert definitions are no longer loaded");
	}

	let root = ctx
		.get::<crate::actions::tamanu::TamanuArgs>()
		.and_then(|t| t.root.clone());
	let install: Option<(Version, PathBuf)> = bestool_tamanu::try_find_tamanu(root.as_deref()).await?;

	// A real install, a DB-only context synthesised from `TAMANU_DATABASE_URL`,
	// or `None`. The daemon still runs and posts sweeps in every case; with
	// `None`, Tamanu/DB checks are skipped, but with only a database URL the DB
	// checks run while install-dependent ones skip.
	let tamanu = resolve_sweep_tamanu(install)?;
	match &tamanu {
		Some(t) => debug!(has_install = t.has_install, "resolved Tamanu sweep context"),
		None => warn!("no Tamanu install and no TAMANU_DATABASE_URL; Tamanu checks will skip"),
	}

	// A pool error here means postgres is down or unreachable. Don't abort
	// startup over it: the daemon must still run so the `db_connect` check
	// (which connects via `database_url`, not this pool) can report it. The
	// pool is only used for the device-key DB read below, which falls back to
	// the registration anyway.
	let pg_pool = match &tamanu {
		Some(t) => match bestool_postgres::pool::create_pool(&t.database_url, "bestool-alertd").await
		{
			Ok(pool) => Some(pool),
			Err(err) => {
				warn!(%err, "postgres not reachable at startup; db_connect will report it");
				None
			}
		},
		None => None,
	};

	let watchdog = if no_watchdog {
		None
	} else {
		Some(std::time::Duration::from_secs(watchdog_timeout))
	};

	let device_key_pem = fetch_device_key_with(|| async {
		match &pg_pool {
			Some(pool) => match pool.get().await {
				Ok(conn) => query_device_key_row(&conn).await,
				Err(err) => {
					warn!(%err, "could not get DB conn for deviceKey fetch");
					None
				}
			},
			None => None,
		}
	})
	.await;

	// Canopy requires a version on every request; `0.0.0` is the agreed
	// sentinel for hosts with no Tamanu.
	let tamanu_version = tamanu
		.as_ref()
		.map(|t| t.version.to_string())
		.unwrap_or_else(|| "0.0.0".into());

	let base = bestool_alertd::DaemonConfig::new(
		pg_pool.clone(),
		tamanu.as_ref().map(|t| t.database_url.clone()),
		tamanu_version,
	)
	.with_binary_version(env!("CARGO_PKG_VERSION").to_string())
	.with_no_server(no_server)
	.with_server_addrs(server_addr)
	.with_watchdog_timeout(watchdog);
	let doctor = doctor_task(env!("CARGO_PKG_VERSION").to_string(), tamanu);
	let mut daemon_config = with_daemon_tasks(base, doctor);

	if let Some(pem) = device_key_pem {
		daemon_config = daemon_config.with_device_key_pem(pem);
	}

	Ok(daemon_config)
}

/// Build the daemon config without any Tamanu integration (this build has no
/// Tamanu support). The daemon still runs and posts sweeps; every
/// Tamanu-dependent check is skipped.
#[cfg(not(feature = "alertd-tamanu"))]
async fn build_config(_ctx: &Context, daemon: DaemonArgs) -> Result<bestool_alertd::DaemonConfig> {
	let DaemonArgs {
		glob,
		no_server,
		server_addr,
		watchdog_timeout,
		no_watchdog,
	} = daemon;
	if !glob.is_empty() {
		warn!("--glob is deprecated and does nothing; alert definitions are no longer loaded");
	}
	warn!("this build has no Tamanu support; doctor sweeps will skip Tamanu checks");

	let watchdog = if no_watchdog {
		None
	} else {
		Some(std::time::Duration::from_secs(watchdog_timeout))
	};

	// The device key (for mTLS to canopy) always comes from the canopy
	// registration — enrolled via `bestool canopy register`, not Tamanu.
	let device_key_pem = bestool_canopy::registration::load()
		.await
		.ok()
		.flatten()
		.and_then(|reg| reg.device_key);

	// Canopy requires a version on every request; `0.0.0` is the agreed
	// sentinel for hosts with no Tamanu.
	let base = bestool_alertd::DaemonConfig::new(None, None, "0.0.0".to_string())
		.with_binary_version(env!("CARGO_PKG_VERSION").to_string())
		.with_no_server(no_server)
		.with_server_addrs(server_addr)
		.with_watchdog_timeout(watchdog);
	let doctor = doctor_task(env!("CARGO_PKG_VERSION").to_string(), None);
	let mut daemon_config = with_daemon_tasks(base, doctor);
	if let Some(pem) = device_key_pem {
		daemon_config = daemon_config.with_device_key_pem(pem);
	}
	Ok(daemon_config)
}
