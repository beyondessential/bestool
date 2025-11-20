use std::path::PathBuf;

use clap::{Parser, Subcommand};
use miette::{Result, miette};
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
		server_addr: Vec<std::net::SocketAddr>,
	},
}

fn default_server_addrs() -> Vec<std::net::SocketAddr> {
	vec![
		"[::1]:8271".parse().unwrap(),
		"127.0.0.1:8271".parse().unwrap(),
	]
}

async fn try_connect_daemon(addrs: &[std::net::SocketAddr]) -> Result<(reqwest::Client, String)> {
	let client = reqwest::Client::new();
	let mut last_error = None;

	for addr in addrs {
		let url = format!("http://{}", addr);
		info!("trying to connect to daemon at {}", url);

		// Try to connect with a simple status check
		let test_response = match client.get(format!("{}/status", url)).send().await {
			Ok(resp) => resp,
			Err(e) => {
				info!("failed to connect to {}: {}", url, e);
				last_error = Some(e);
				continue;
			}
		};

		if test_response.status().is_success() {
			info!("connected to daemon at {}", url);
			return Ok((client, url));
		}
	}

	if let Some(err) = last_error {
		Err(miette!(
			"failed to connect to daemon at any of {} address(es): {}",
			addrs.len(),
			err
		))
	} else {
		Err(miette!(
			"no daemon found at any of {} address(es)",
			addrs.len()
		))
	}
}

pub async fn run(ctx: Context<TamanuArgs, AlertdArgs>) -> Result<()> {
	match ctx.args_sub.command {
		Command::Reload { server_addr } => {
			let addrs = if server_addr.is_empty() {
				default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::send_reload(&addrs).await
		}
		Command::LoadedAlerts { server_addr } => {
			let addrs = if server_addr.is_empty() {
				default_server_addrs()
			} else {
				server_addr
			};
			get_loaded_alerts(&addrs).await
		}
		Command::PauseAlert {
			alert,
			until,
			server_addr,
		} => {
			let addrs = if server_addr.is_empty() {
				default_server_addrs()
			} else {
				server_addr
			};
			pause_alert(&alert, until.as_deref(), &addrs).await
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
	let (client, base_url) = try_connect_daemon(addrs).await?;

	let response = client
		.get(format!("{}/alerts", base_url))
		.send()
		.await
		.map_err(|e| miette!("failed to get alerts: {}", e))?;

	if !response.status().is_success() {
		return Err(miette!(
			"failed to get alerts (status: {})",
			response.status()
		));
	}

	let alerts: Vec<String> = response
		.json()
		.await
		.map_err(|e| miette!("failed to parse response: {}", e))?;

	if alerts.is_empty() {
		println!("No alerts currently loaded");
	} else {
		println!("Loaded alerts ({}):", alerts.len());
		for alert in alerts {
			println!("  {}", alert);
		}
	}

	Ok(())
}

async fn pause_alert(
	alert_path: &str,
	until: Option<&str>,
	addrs: &[std::net::SocketAddr],
) -> Result<()> {
	use std::io::{self, Write};

	// Parse or default the until time
	let until_timestamp = if let Some(until_str) = until {
		// Try parsing as timestamp first
		if let Ok(ts) = until_str.parse::<jiff::Timestamp>() {
			ts
		} else {
			// Try parsing as relative time using jiff's Span
			let span: jiff::Span = until_str
				.parse()
				.map_err(|e| miette!("failed to parse time '{}': {}", until_str, e))?;
			jiff::Timestamp::now()
				.checked_add(span)
				.map_err(|e| miette!("time calculation overflow: {}", e))?
		}
	} else {
		// Default to 1 week from now
		jiff::Timestamp::now()
			.checked_add(jiff::Span::new().days(7))
			.map_err(|e| miette!("time calculation overflow: {}", e))?
	};

	let (client, base_url) = try_connect_daemon(addrs).await?;

	// Try to pause the alert
	let url = format!("{}/alerts", base_url);

	let body = serde_json::json!({
		"alert": alert_path,
		"until": until_timestamp.to_string(),
	});

	let response = client
		.delete(&url)
		.json(&body)
		.send()
		.await
		.map_err(|e| miette!("failed to send pause request: {}", e))?;

	if response.status() == reqwest::StatusCode::NOT_FOUND {
		// Alert not found, try to find a partial match
		info!("alert not found, trying to find partial match");

		let alerts_response = client
			.get(format!("{}/alerts", base_url))
			.send()
			.await
			.map_err(|e| miette!("failed to get alerts list: {}", e))?;

		let alerts: Vec<String> = alerts_response
			.json()
			.await
			.map_err(|e| miette!("failed to parse alerts list: {}", e))?;

		// Find partial matches
		let matches: Vec<&String> = alerts.iter().filter(|a| a.contains(alert_path)).collect();

		if matches.is_empty() {
			return Err(miette!(
				"alert '{}' not found and no partial matches",
				alert_path
			));
		} else if matches.len() == 1 {
			// Exactly one match, ask for confirmation
			println!("Alert '{}' not found.", alert_path);
			println!("Did you mean: {}", matches[0]);
			print!("Pause this alert? [y/N] ");
			io::stdout().flush().unwrap();

			let mut input = String::new();
			io::stdin()
				.read_line(&mut input)
				.map_err(|e| miette!("failed to read input: {}", e))?;

			if input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes") {
				// Retry with the matched path
				let retry_url = format!("{}/alerts", base_url);
				let retry_body = serde_json::json!({
					"alert": matches[0],
					"until": until_timestamp.to_string(),
				});

				let retry_response = client
					.delete(&retry_url)
					.json(&retry_body)
					.send()
					.await
					.map_err(|e| miette!("failed to send pause request: {}", e))?;

				if !retry_response.status().is_success() {
					return Err(miette!(
						"failed to pause alert (status: {})",
						retry_response.status()
					));
				}

				println!("Alert paused until {}", until_timestamp);
				return Ok(());
			} else {
				return Err(miette!("pause cancelled"));
			}
		} else {
			// Multiple matches
			println!(
				"Alert '{}' not found. Did you mean one of these?",
				alert_path
			);
			for (i, m) in matches.iter().enumerate() {
				println!("  {}. {}", i + 1, m);
			}
			return Err(miette!("multiple matches found, please be more specific"));
		}
	}

	if !response.status().is_success() {
		return Err(miette!(
			"failed to pause alert (status: {})",
			response.status()
		));
	}

	println!("Alert paused until {}", until_timestamp);
	Ok(())
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
