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

	let mut daemon_config = bestool_alertd::DaemonConfig::new(
		pg_pool.clone(),
		tamanu.as_ref().map(|t| t.database_url.clone()),
		tamanu_version,
	)
	.with_no_server(no_server)
	.with_server_addrs(server_addr)
	.with_watchdog_timeout(watchdog)
	.with_task(Arc::new(DoctorTask::new(
		env!("CARGO_PKG_VERSION").to_string(),
		tamanu,
	)));

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
	let mut daemon_config = bestool_alertd::DaemonConfig::new(None, None, "0.0.0".to_string())
		.with_no_server(no_server)
		.with_server_addrs(server_addr)
		.with_watchdog_timeout(watchdog)
		.with_task(Arc::new(DoctorTask::new(
			env!("CARGO_PKG_VERSION").to_string(),
			None,
		)));
	if let Some(pem) = device_key_pem {
		daemon_config = daemon_config.with_device_key_pem(pem);
	}
	Ok(daemon_config)
}
