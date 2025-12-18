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
///
/// The alert and target definitions are documented online at:
/// <https://github.com/beyondessential/bestool/blob/main/crates/alertd/ALERTS.md>
/// and <https://github.com/beyondessential/bestool/blob/main/crates/alertd/TARGETS.md>.
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

	/// Generate markdown documentation
	#[command(hide = true, name = "_docs")]
	Docs,
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
		Command::Validate { file, server_addr } => {
			let addrs = if server_addr.is_empty() {
				bestool_alertd::commands::default_server_addrs()
			} else {
				server_addr
			};
			bestool_alertd::commands::validate_alert(&file, &addrs).await
		}
		#[cfg(windows)]
		Command::Install => bestool_alertd::windows_service::install_service(),
		#[cfg(windows)]
		Command::Uninstall => bestool_alertd::windows_service::uninstall_service(),
		#[cfg(windows)]
		Command::Service { daemon } => {
			let daemon_config = build_daemon_config(daemon)?;
			bestool_alertd::windows_service::run_service(daemon_config)
		}
		Command::Docs => {
			let markdown = clap_markdown::help_markdown::<Args>();
			println!("{}", markdown);
			Ok(())
		}
	}
}
