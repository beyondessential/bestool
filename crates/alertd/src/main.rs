use clap::{Parser, Subcommand};
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{Result, miette};
use tracing::debug;

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

#[tokio::main]
async fn main() -> Result<()> {
	let (args, _guard) = get_args()?;

	match args.command {
		Command::Run { daemon } => run_daemon(daemon).await,
		Command::Reload { server_addr } => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::send_reload(&addrs).await
		}
		Command::LoadedAlerts { server_addr } => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::get_loaded_alerts(&addrs).await
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
		Command::Validate { file, server_addr } => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::validate_alert(&file, &addrs).await
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
