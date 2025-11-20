use std::path::PathBuf;

use clap::{Parser, Subcommand};
use miette::Result;
use tracing::{debug, info};

use super::{TamanuArgs, config::load_config, connection_url::ConnectionUrlBuilder, find_tamanu};
use crate::actions::Context;

/// Run the alert daemon
///
/// This is a long-lived daemon that manages alert execution on scheduled intervals.
/// Unlike the `alerts` subcommand which is designed to run via cron, this daemon
/// manages its own timers and watches for configuration file changes.
///
/// Configuration for database and email is read from Tamanu's config files.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct AlertdArgs {
	#[command(subcommand)]
	command: Command,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
	/// Run the alert daemon
	///
	/// Starts the daemon which monitors alert definition files and executes alerts
	/// based on their configured schedules. The daemon will watch for file changes
	/// and automatically reload when definitions are modified.
	Run {
		/// Glob patterns for alert definitions
		///
		/// Patterns can match directories (which will be read recursively) or individual files.
		/// Can be provided multiple times.
		/// Examples: /etc/tamanu/alerts, /opt/*/alerts, /etc/tamanu/alerts/**/*.yml
		#[arg(long)]
		dir: Vec<String>,

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
		server_addr: Vec<std::net::SocketAddr>,
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
		server_addr: Vec<std::net::SocketAddr>,
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
		server_addr: Vec<std::net::SocketAddr>,
	},
}

pub async fn run(ctx: Context<TamanuArgs, AlertdArgs>) -> Result<()> {
	match ctx.args_sub.command {
		Command::Reload { server_addr } => {
			let addrs = if server_addr.is_empty() {
				vec![
					"[::1]:8271".parse().unwrap(),
					"127.0.0.1:8271".parse().unwrap(),
				]
			} else {
				server_addr
			};
			bestool_alertd::send_reload(&addrs).await
		}
		Command::LoadedAlerts { server_addr } => {
			let addrs = if server_addr.is_empty() {
				vec![
					"[::1]:8271".parse().unwrap(),
					"127.0.0.1:8271".parse().unwrap(),
				]
			} else {
				server_addr
			};
			get_loaded_alerts(&addrs).await
		}
		Command::Run {
			dir,
			dry_run,
			no_server,
			server_addr,
		} => {
			let (_, root) = find_tamanu(&ctx.args_top)?;
			let config = load_config(&root, None)?;
			debug!(?config, "parsed Tamanu config");

			let dirs = if dir.is_empty() {
				default_dirs(&root).await
			} else {
				dir
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

			bestool_alertd::run(daemon_config).await
		}
	}
}

async fn get_loaded_alerts(addrs: &[std::net::SocketAddr]) -> Result<()> {
	let client = reqwest::Client::new();
	let mut last_error = None;

	for addr in addrs {
		let url = format!("http://{}/alerts", addr);
		info!("querying daemon at {}", url);

		let response = match client.get(&url).send().await {
			Ok(resp) => resp,
			Err(e) => {
				info!("failed to connect to {}: {}", url, e);
				last_error = Some(e);
				continue;
			}
		};

		if !response.status().is_success() {
			info!("daemon at {} returned status: {}", url, response.status());
			continue;
		}

		let alerts: Vec<String> = match response.json().await {
			Ok(a) => a,
			Err(e) => {
				info!("failed to parse response from {}: {}", url, e);
				continue;
			}
		};

		if alerts.is_empty() {
			println!("No alerts currently loaded");
		} else {
			println!("Loaded alerts ({}):", alerts.len());
			for alert in alerts {
				println!("  {}", alert);
			}
		}

		return Ok(());
	}

	if let Some(err) = last_error {
		Err(miette::miette!(
			"failed to connect to daemon at any of {} address(es): {}",
			addrs.len(),
			err
		))
	} else {
		Err(miette::miette!(
			"no daemon found at any of {} address(es)",
			addrs.len()
		))
	}
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
