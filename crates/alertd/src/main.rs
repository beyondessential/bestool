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
	#[command(subcommand)]
	command: Option<Command>,

	#[command(flatten)]
	logging: LoggingArgs,

	/// Send reload signal to running daemon and exit
	///
	/// Connects to the running daemon's HTTP API and triggers a reload.
	/// This is an alternative to SIGHUP that works on all platforms including Windows.
	#[arg(long, conflicts_with_all = ["database_url", "glob", "email_from", "mailgun_api_key", "mailgun_domain", "dry_run"])]
	pub reload: bool,

	/// Database connection URL
	///
	/// PostgreSQL connection URL, e.g., postgresql://user:pass@localhost/dbname
	#[arg(long, env = "DATABASE_URL")]
	pub database_url: Option<String>,

	/// Glob patterns for alert definitions
	///
	/// Patterns can match directories (which will be read recursively) or individual files.
	/// Can be provided multiple times.
	/// Examples: /etc/tamanu/alerts, /opt/*/alerts, /etc/tamanu/alerts/**/*.yml
	#[arg(long)]
	pub glob: Vec<String>,

	/// Email sender address
	#[arg(long, env = "EMAIL_FROM")]
	pub email_from: Option<String>,

	/// Mailgun API key
	#[arg(long, env = "MAILGUN_API_KEY")]
	pub mailgun_api_key: Option<String>,

	/// Mailgun domain
	#[arg(long, env = "MAILGUN_DOMAIN")]
	pub mailgun_domain: Option<String>,

	/// Execute all alerts once and quit (ignoring intervals)
	#[arg(long)]
	pub dry_run: bool,

	/// Disable the HTTP server
	#[arg(long, conflicts_with = "reload")]
	pub no_server: bool,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
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
	/// Run as a Windows service (used by Windows Service Manager)
	///
	/// This command is invoked by the Windows Service Control Manager and should
	/// not be called directly. Use 'install' to set up the service, then manage
	/// it through Windows service management tools.
	Service,
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

#[tokio::main]
async fn main() -> Result<()> {
	let (args, _guard) = get_args()?;

	#[cfg(windows)]
	if let Some(command) = args.command {
		return match command {
			Command::Install => install_service(),
			Command::Uninstall => uninstall_service(),
			Command::Service => {
				let database_url = args
					.database_url
					.ok_or_else(|| miette!("--database-url is required"))?;

				if args.glob.is_empty() {
					return Err(miette!("at least one --glob must be specified"));
				}

				let email = match (args.email_from, args.mailgun_api_key, args.mailgun_domain) {
					(Some(from), Some(api_key), Some(domain)) => {
						Some(bestool_alertd::EmailConfig {
							from,
							mailgun_api_key: api_key,
							mailgun_domain: domain,
						})
					}
					(None, None, None) => None,
					_ => {
						return Err(miette!(
							"either provide all email options (--email-from, --mailgun-api-key, --mailgun-domain) or none"
						));
					}
				};

				let mut daemon_config = bestool_alertd::DaemonConfig::new(args.glob, database_url)
					.with_dry_run(args.dry_run)
					.with_no_server(args.no_server);

				if let Some(email) = email {
					daemon_config = daemon_config.with_email(email);
				}

				return bestool_alertd::windows_service::run_service(daemon_config);
			}
		};
	}

	if args.reload {
		return bestool_alertd::send_reload().await;
	}

	let database_url = args
		.database_url
		.ok_or_else(|| miette!("--database-url is required"))?;

	if args.glob.is_empty() {
		return Err(miette!("at least one --glob must be specified"));
	}

	let email = match (args.email_from, args.mailgun_api_key, args.mailgun_domain) {
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

	let mut daemon_config = bestool_alertd::DaemonConfig::new(args.glob, database_url)
		.with_dry_run(args.dry_run)
		.with_no_server(args.no_server);

	if let Some(email) = email {
		daemon_config = daemon_config.with_email(email);
	}

	bestool_alertd::run(daemon_config).await
}
