use clap::{Parser, Subcommand};
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{Result, miette};
use tracing::{debug, info, warn};

/// BES tooling: Alert daemon
///
/// The daemon watches for changes to alert definition files and automatically reloads
/// when changes are detected. You can also send SIGHUP to manually trigger a reload.
///
/// On Windows, the daemon can be installed as a native Windows service using the
/// 'install' subcommand. See 'bestool-alertd install --help' for details.
#[derive(Debug, Clone, Parser)]
pub struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	#[command(subcommand)]
	command: Command,
}

/// Common arguments for running the daemon
#[derive(Debug, Clone, Parser)]
struct DaemonArgs {
	/// Database connection URL
	///
	/// PostgreSQL connection URL, e.g., postgresql://user:pass@localhost/dbname
	#[arg(long, env = "DATABASE_URL")]
	database_url: Option<String>,

	/// Glob patterns for alert definitions
	///
	/// Patterns can match directories (which will be read recursively) or individual files.
	/// Can be provided multiple times.
	/// Examples: /etc/tamanu/alerts, /opt/*/alerts, /etc/tamanu/alerts/**/*.yml
	#[arg(long)]
	glob: Vec<String>,

	/// Email sender address
	#[arg(long, env = "EMAIL_FROM")]
	email_from: Option<String>,

	/// Mailgun API key
	#[arg(long, env = "MAILGUN_API_KEY")]
	mailgun_api_key: Option<String>,

	/// Mailgun domain
	#[arg(long, env = "MAILGUN_DOMAIN")]
	mailgun_domain: Option<String>,

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

	/// Validate an alert definition file
	///
	/// Parses an alert definition file and reports any syntax or validation errors.
	/// Uses pretty error reporting to pinpoint the exact location of problems.
	/// Requires the daemon to be running.
	Validate {
		/// Path to the alert definition file to validate
		file: std::path::PathBuf,

		/// HTTP server address(es) to try
		///
		/// Can be provided multiple times. Will attempt to connect to each address
		/// in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
		#[arg(long)]
		server_addr: Vec<std::net::SocketAddr>,
	},

	#[cfg(windows)]
	/// Install the daemon as a Windows service
	///
	/// Creates a Windows service named 'bestool-alertd' that will start automatically.
	/// After installation, configure the service with environment variables or command
	/// line arguments, then start it with: sc start bestool-alertd
	Install,

	#[cfg(windows)]
	/// Uninstall the Windows service
	///
	/// Removes the 'bestool-alertd' Windows service. The service must be stopped
	/// before uninstallation. Use: sc stop bestool-alertd
	Uninstall,

	#[cfg(windows)]
	#[command(hide = true)]
	Service {
		#[command(flatten)]
		daemon: DaemonArgs,
	},
}

async fn validate_alert(file: &std::path::Path, addrs: &[std::net::SocketAddr]) -> Result<()> {
	use miette::{Context as _, IntoDiagnostic, NamedSource, SourceSpan};

	// Read the file
	let content = std::fs::read_to_string(file)
		.into_diagnostic()
		.wrap_err_with(|| format!("failed to read file: {}", file.display()))?;

	// Connect to daemon
	let (client, base_url) = try_connect_daemon(addrs).await?;

	// Check daemon version
	let status_response = client
		.get(format!("{}/status", base_url))
		.send()
		.await
		.into_diagnostic()
		.wrap_err("failed to get daemon status")?;

	#[derive(serde::Deserialize)]
	struct StatusResponse {
		version: String,
	}

	if let Ok(status) = status_response.json::<StatusResponse>().await {
		let daemon_version = &status.version;
		let cli_version = bestool_alertd::VERSION;
		if daemon_version != cli_version {
			warn!(
				"version mismatch: daemon is running {} but CLI is {}",
				daemon_version, cli_version
			);
			eprintln!(
				"⚠ Warning: Version mismatch detected!\n  Daemon version: {}\n  CLI version: {}\n",
				daemon_version, cli_version
			);
		}
	}

	// Send validation request
	let response = client
		.post(format!("{}/validate", base_url))
		.body(content.clone())
		.send()
		.await
		.into_diagnostic()
		.wrap_err("failed to send validation request")?;

	if !response.status().is_success() {
		return Err(miette!(
			"validation request failed with status: {}",
			response.status()
		));
	}

	// Parse response
	#[derive(serde::Deserialize)]
	struct ValidationResponse {
		valid: bool,
		error: Option<String>,
		error_location: Option<ErrorLocation>,
		info: Option<ValidationInfo>,
	}

	#[derive(serde::Deserialize)]
	struct ErrorLocation {
		line: usize,
		column: usize,
		path: String,
	}

	#[derive(serde::Deserialize)]
	struct ValidationInfo {
		enabled: bool,
		interval: String,
		source_type: String,
		targets: usize,
	}

	let validation: ValidationResponse = response
		.json()
		.await
		.into_diagnostic()
		.wrap_err("failed to parse validation response")?;

	if validation.valid {
		println!("✓ Alert definition is valid");
		println!("  File: {}", file.display());
		if let Some(info) = validation.info {
			println!("  Enabled: {}", info.enabled);
			println!("  Interval: {}", info.interval);
			println!("  Source: {}", info.source_type);
			println!("  Targets: {}", info.targets);

			if info.targets == 0 {
				println!("\n⚠ Warning: Alert has no resolved targets.");
				println!("  This alert may not send notifications. Check your _targets.yml file.");
			}
		}
		Ok(())
	} else {
		// Display error with source location if available
		if let Some(error_msg) = validation.error {
			if let Some(loc) = validation.error_location {
				// Calculate byte offset for miette
				let mut byte_offset = 0;
				for (idx, line_content) in content.lines().enumerate() {
					if idx + 1 < loc.line {
						byte_offset += line_content.len() + 1; // +1 for newline
					} else if idx + 1 == loc.line {
						byte_offset += loc.column.saturating_sub(1);
						break;
					}
				}

				let span_start = byte_offset;
				let span_len = content[span_start..]
					.lines()
					.next()
					.map(|l| l.len().min(80))
					.unwrap_or(1);

				Err(miette!(
					labels = vec![miette::LabeledSpan::at(
						SourceSpan::new(span_start.into(), span_len.into()),
						"here"
					)],
					"{}",
					error_msg
				)
				.with_source_code(NamedSource::new(file.display().to_string(), content)))
			} else {
				Err(miette!("{}", error_msg)
					.with_source_code(NamedSource::new(file.display().to_string(), content)))
			}
		} else {
			Err(miette!("validation failed with no error message"))
		}
	}
}

fn get_args() -> Result<(Args, WorkerGuard)> {
	let log_guard = PreArgs::parse().setup().map_err(|err| miette!("{err}"))?;

	debug!("parsing arguments");
	let args = Args::parse();

	let log_guard = match log_guard {
		Some(g) => g,
		None => args
			.logging
			.setup(|v| match v {
				0 => "bestool_alertd=info",
				1 => "info,bestool_alertd=debug",
				2 => "debug",
				3 => "debug,bestool_alertd=trace",
				_ => "trace",
			})
			.map_err(|err| miette!("{err}"))?,
	};

	debug!(?args, "got arguments");
	Ok((args, log_guard))
}

#[cfg(windows)]
fn install_service() -> Result<()> {
	use std::ffi::OsString;
	use windows_service::{
		service::{ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType},
		service_manager::{ServiceManager, ServiceManagerAccess},
	};

	let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
	let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
		.map_err(|e| miette!("failed to connect to service manager: {e}"))?;

	let service_binary_path = std::env::current_exe()
		.map_err(|e| miette!("failed to get current executable path: {e}"))?;

	let service_info = ServiceInfo {
		name: OsString::from("bestool-alertd"),
		display_name: OsString::from("BES Alert Daemon"),
		service_type: ServiceType::OWN_PROCESS,
		start_type: ServiceStartType::AutoStart,
		error_control: ServiceErrorControl::Normal,
		executable_path: service_binary_path,
		launch_arguments: vec![OsString::from("service")],
		dependencies: vec![],
		account_name: None,
		account_password: None,
	};

	let service = service_manager
		.create_service(&service_info, ServiceAccess::CHANGE_CONFIG)
		.map_err(|e| miette!("failed to create service: {e}"))?;

	service
		.set_description("Monitors and executes alert definitions from configuration files")
		.map_err(|e| miette!("failed to set service description: {e}"))?;

	println!("Service installed successfully");
	println!("Configure the service with environment variables or registry settings");
	println!("Start the service with: sc start bestool-alertd");
	Ok(())
}

#[cfg(windows)]
fn uninstall_service() -> Result<()> {
	use windows_service::{
		service::ServiceAccess,
		service_manager::{ServiceManager, ServiceManagerAccess},
	};

	let manager_access = ServiceManagerAccess::CONNECT;
	let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
		.map_err(|e| miette!("failed to connect to service manager: {e}"))?;

	let service_access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
	let service = service_manager
		.open_service("bestool-alertd", service_access)
		.map_err(|e| miette!("failed to open service: {e}"))?;

	service
		.delete()
		.map_err(|e| miette!("failed to delete service: {e}"))?;

	println!("Service uninstalled successfully");
	Ok(())
}

fn build_daemon_config(daemon: DaemonArgs) -> Result<bestool_alertd::DaemonConfig> {
	let database_url = daemon
		.database_url
		.ok_or_else(|| miette!("--database-url is required"))?;

	if daemon.glob.is_empty() {
		return Err(miette!("at least one --glob must be specified"));
	}

	let email = match (
		daemon.email_from,
		daemon.mailgun_api_key,
		daemon.mailgun_domain,
	) {
		(Some(from), Some(api_key), Some(domain)) => Some(bestool_alertd::EmailConfig {
			from,
			mailgun_api_key: api_key,
			mailgun_domain: domain,
		}),
		(None, None, None) => None,
		_ => {
			return Err(miette!(
				"either provide all email options (--email-from, --mailgun-api-key, --mailgun-domain) or none"
			));
		}
	};

	let mut daemon_config = bestool_alertd::DaemonConfig::new(daemon.glob, database_url)
		.with_dry_run(daemon.dry_run)
		.with_no_server(daemon.no_server)
		.with_server_addrs(daemon.server_addr);

	if let Some(email) = email {
		daemon_config = daemon_config.with_email(email);
	}

	Ok(daemon_config)
}

async fn run_daemon(daemon: DaemonArgs) -> Result<()> {
	let daemon_config = build_daemon_config(daemon)?;
	bestool_alertd::run(daemon_config).await
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

#[tokio::main]
async fn main() -> Result<()> {
	let (args, _guard) = get_args()?;

	match args.command {
		Command::Run { daemon } => run_daemon(daemon).await,
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
		Command::Validate { file, server_addr } => {
			let addrs = if server_addr.is_empty() {
				default_server_addrs()
			} else {
				server_addr
			};
			validate_alert(&file, &addrs).await
		}
		#[cfg(windows)]
		Command::Install => install_service(),
		#[cfg(windows)]
		Command::Uninstall => uninstall_service(),
		#[cfg(windows)]
		Command::Service { daemon } => {
			let daemon_config = build_daemon_config(daemon)?;
			bestool_alertd::windows_service::run_service(daemon_config)
		}
	}
}
