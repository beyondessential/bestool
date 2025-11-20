use clap::Parser;
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{Result, miette};
use tracing::debug;

/// BES tooling: Alert daemon
///
/// The daemon watches for changes to alert definition files and automatically reloads
/// when changes are detected. You can also send SIGHUP to manually trigger a reload.
#[derive(Debug, Clone, Parser)]
pub struct Args {
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

#[tokio::main]
async fn main() -> Result<()> {
	let (args, _guard) = get_args()?;

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

	let mut daemon_config =
		bestool_alertd::DaemonConfig::new(args.glob, database_url).with_dry_run(args.dry_run);

	if let Some(email) = email {
		daemon_config = daemon_config.with_email(email);
	}

	bestool_alertd::run(daemon_config).await
}
