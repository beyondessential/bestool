use std::{net::SocketAddr, path::Path, sync::Arc};

use clap::{Parser, Subcommand};
use miette::Result;
use tracing::{debug, warn};

use bestool_tamanu::{
	config::{TamanuConfig, load_config},
	server_info::{fetch_device_key_with, query_device_key_row},
};

use bestool_alertd::doctor::DoctorTask;

use super::{TamanuArgs, find_tamanu};
use crate::actions::Context;

/// Run the healthcheck daemon
///
/// Periodically runs the doctor healthcheck sweep and posts the result to
/// canopy. Database and device-key configuration is read from Tamanu's config
/// files.
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
			let (version, root) = find_tamanu(ctx.require::<TamanuArgs>())?;
			let config = load_config(&root, None)?;
			debug!(?config, "parsed Tamanu config");

			let daemon_config = build_config(&root, &version, config, daemon).await?;
			bestool_alertd::run(daemon_config).await
		}
		#[cfg(windows)]
		Command::Install => {
			use std::ffi::OsString;
			bestool_alertd::windows_service::install_service_with_args(&[
				OsString::from("tamanu"),
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
			let (version, root) = find_tamanu(ctx.require::<TamanuArgs>())?;
			let config = load_config(&root, None)?;
			debug!(?config, "parsed Tamanu config");

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

			let daemon_config = build_config(&root, &version, config, daemon).await?;
			bestool_alertd::windows_service::run_service(daemon_config)
		}
	}
}

async fn build_config(
	root: &Path,
	tamanu_version: &node_semver::Version,
	config: TamanuConfig,
	DaemonArgs {
		glob,
		no_server,
		server_addr,
		watchdog_timeout,
		no_watchdog,
	}: DaemonArgs,
) -> Result<bestool_alertd::DaemonConfig> {
	if !glob.is_empty() {
		warn!("--glob is deprecated and does nothing; alert definitions are no longer loaded");
	}

	let database_url = config.database_url();
	let pg_pool = bestool_postgres::pool::create_pool(&database_url, "bestool-alertd").await?;

	let watchdog = if no_watchdog {
		None
	} else {
		Some(std::time::Duration::from_secs(watchdog_timeout))
	};

	let device_key_pem = fetch_device_key_with(|| async {
		match pg_pool.get().await {
			Ok(conn) => query_device_key_row(&conn).await,
			Err(err) => {
				tracing::warn!(%err, "could not get DB conn for deviceKey fetch");
				None
			}
		}
	})
	.await;

	let config = Arc::new(config);

	let mut daemon_config = bestool_alertd::DaemonConfig::new(
		pg_pool.clone(),
		database_url.clone(),
		tamanu_version.to_string(),
	)
	.with_no_server(no_server)
	.with_server_addrs(server_addr)
	.with_watchdog_timeout(watchdog)
	.with_task(Arc::new(DoctorTask::new(
		env!("CARGO_PKG_VERSION").to_string(),
		tamanu_version.clone(),
		root.to_path_buf(),
		config.clone(),
		database_url,
	)));

	if let Some(pem) = device_key_pem {
		daemon_config = daemon_config.with_device_key_pem(pem);
	}

	Ok(daemon_config)
}
