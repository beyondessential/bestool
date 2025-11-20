use std::path::PathBuf;

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

	/// Database connection URL
	///
	/// PostgreSQL connection URL, e.g., postgresql://user:pass@localhost/dbname
	#[arg(long, env = "DATABASE_URL")]
	pub database_url: String,

	/// Folder containing alert definitions
	///
	/// This folder will be read recursively for files with the `.yaml` or `.yml` extension.
	/// Can be provided multiple times.
	#[arg(long)]
	pub dir: Vec<PathBuf>,

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

	if args.dir.is_empty() {
		return Err(miette!("at least one --dir must be specified"));
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
		bestool_alertd::DaemonConfig::new(args.dir, args.database_url).with_dry_run(args.dry_run);

	if let Some(email) = email {
		daemon_config = daemon_config.with_email(email);
	}

	bestool_alertd::run(daemon_config).await
}
