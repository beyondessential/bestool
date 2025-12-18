use std::{path::{Path, PathBuf}, net::SocketAddr};

use clap::{Parser, Subcommand};
use miette::Result;
use tracing::{debug, info};

use super::{TamanuArgs, config::{TamanuConfig, load_config}, connection_url::ConnectionUrlBuilder, find_tamanu};
use crate::actions::Context;

/// Run the alert daemon
///
/// The alert and target definitions are documented online at:
/// <https://github.com/beyondessential/bestool/blob/main/crates/alertd/ALERTS.md>
/// and <https://github.com/beyondessential/bestool/blob/main/crates/alertd/TARGETS.md>.
///
/// Configuration for database and email is read from Tamanu's config files.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct AlertdArgs {
	#[command(subcommand)]
	command: Command,
}

/// Common arguments for running the daemon
#[derive(Debug, Clone, Parser)]
struct DaemonArgs {
	/// Glob patterns for alert definitions
	///
	/// Patterns can match directories (which will be read recursively) or individual files.
	/// Can be provided multiple times.
	/// Examples: /etc/tamanu/alerts, /opt/*/alerts, /etc/tamanu/alerts/**/*.yml
	#[arg(long)]
	glob: Vec<String>,

	/// Execute all alerts once and quit (ignoring intervals)
	#[arg(long)]
	dry_run: bool,

	/// Disable the HTTP server
	#[arg(long)]
	no_server: bool,

	/// HTTP server bind address(es)
	///
	/// Can be provided multiple times. The server will attempt to bind to each address
	/// in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
	#[arg(long)]
	server_addr: Vec<SocketAddr>,
}


#[derive(Debug, Clone, Subcommand)]
enum Command {
	/// Run the alert daemon
	///
	/// Starts the daemon which monitors alert definition files and executes alerts
	/// based on their configured schedules. The daemon will watch for file changes
	/// and automatically reload when definitions are modified.
	Run {
		#[command(flatten)]
		daemon: DaemonArgs,
	},

	/// Send reload signal to running daemon
	///
	/// Connects to the running daemon's HTTP API and triggers a reload.
	/// This is an alternative to SIGHUP that works on all platforms including Windows.
	Reload {
		/// HTTP server address(es) to try
		///
		/// Can be provided multiple times. Will attempt to connect to each address
		/// in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
		#[arg(long)]
		server_addr: Vec<SocketAddr>,
	},

	/// List currently loaded alert files
	///
	/// Connects to the running daemon's HTTP API and retrieves the list of
	/// currently loaded alert definition files.
	LoadedAlerts {
		/// HTTP server address(es) to try
		///
		/// Can be provided multiple times. Will attempt to connect to each address
		/// in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
		#[arg(long)]
		server_addr: Vec<SocketAddr>,

		/// Show detailed state information for each alert
		#[arg(long)]
		detail: bool,
	},

	/// Temporarily pause an alert
	///
	/// Pauses an alert until the specified time. The alert will not execute during
	/// this period. The pause is lost when the daemon restarts.
	PauseAlert {
		/// Alert file path to pause
		alert: String,

		/// Time until which to pause the alert (fuzzy time format)
		///
		/// Examples: "1 hour", "2 days", "next monday", "2024-12-25T10:00:00Z"
		/// Defaults to 1 week from now if not specified.
		#[arg(long)]
		until: Option<String>,

		/// HTTP server address(es) to try
		///
		/// Can be provided multiple times. Will attempt to connect to each address
		/// in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
		#[arg(long)]
		server_addr: Vec<SocketAddr>,
	},

	/// Validate an alert definition file
	///
	/// Parses an alert definition file and reports any syntax or validation errors.
	/// Uses pretty error reporting to pinpoint the exact location of problems.
	/// Requires the daemon to be running.
	Validate {
		/// Path to the alert definition file to validate
		file: PathBuf,

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

	#[cfg(windows)]
	#[command(hide = true)]
	Service {
		#[command(flatten)]
		daemon: DaemonArgs,
	},
}

pub async fn run(ctx: Context<TamanuArgs, AlertdArgs>) -> Result<()> {
	match ctx.args_sub.command {
		Command::Validate { file, server_addr } => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::validate_alert(&file, &addrs).await
		}
		Command::Reload { server_addr } => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::send_reload(&addrs).await
		}
		Command::LoadedAlerts {
			server_addr,
			detail,
		} => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::get_loaded_alerts(&addrs, detail).await
		}
		Command::PauseAlert {
			alert,
			until,
			server_addr,
		} => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::pause_alert(&alert, until.as_deref(), &addrs).await
		}
		Command::Run {
			daemon,
		} => {
			let (_, root) = find_tamanu(&ctx.args_top)?;
			let config = load_config(&root, None)?;
			debug!(?config, "parsed Tamanu config");

			let daemon_config = build_config(&root, config, daemon).await?;
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
		Command::Service { daemon } => {
			let (_, root) = find_tamanu(&ctx.args_top)?;
			let config = load_config(&root, None)?;
			debug!(?config, "parsed Tamanu config");

			let daemon_config = build_config(&root, config, daemon).await?;
			bestool_alertd::windows_service::run_service(daemon_config)
		}
	}
}

async fn build_config(
	root: &Path,
	config: TamanuConfig,
	DaemonArgs {
		glob,
		dry_run,
		no_server,
		server_addr,
	}: DaemonArgs,
) -> Result<bestool_alertd::DaemonConfig> {
	let dirs = if glob.is_empty() {
		default_dirs(root).await
	} else {
		glob
	};
	debug!(?dirs, "alert directories");

	if dirs.is_empty() {
		return Err(miette::miette!("no alert directories found or specified"));
	}

	info!("starting alertd daemon");

	let database_url = ConnectionUrlBuilder {
		username: config.db.username.clone(),
		password: Some(config.db.password.clone()),
		host: config
			.db
			.host
			.clone()
			.unwrap_or_else(|| "localhost".to_string()),
		port: config.db.port,
		database: config.db.name.clone(),
		ssl_mode: None,
	}
	.build();

	let email = config
		.mailgun
		.as_ref()
		.map(|mg| bestool_alertd::EmailConfig {
			from: mg.sender.clone(),
			mailgun_api_key: mg.api_key.clone(),
			mailgun_domain: mg.domain.clone(),
		});

	let mut daemon_config = bestool_alertd::DaemonConfig::new(dirs, database_url)
		.with_dry_run(dry_run)
		.with_no_server(no_server)
		.with_server_addrs(server_addr);

	if let Some(email) = email {
		daemon_config = daemon_config.with_email(email);
	}

	Ok(daemon_config)
}

async fn default_dirs(root: &std::path::Path) -> Vec<String> {
	use futures::future::join_all;

	let mut dirs = vec![
		PathBuf::from(r"C:\Tamanu\alerts"),
		root.join("alerts"),
		PathBuf::from("/opt/tamanu-toolbox/alerts"),
		PathBuf::from("/etc/tamanu/alerts"),
		PathBuf::from("/alerts"),
	];
	if let Ok(cwd) = std::env::current_dir() {
		dirs.push(cwd.join("alerts"));
	}

	join_all(
		dirs.into_iter()
			.map(|dir| async { if dir.exists() { Some(dir) } else { None } }),
	)
	.await
	.into_iter()
	.flatten()
	.map(|p| p.display().to_string())
	.collect()
}
