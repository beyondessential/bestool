use std::path::PathBuf;

use clap::Parser;
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{IntoDiagnostic, Result, miette};
use tracing::debug;

/// BES tooling: Alert daemon
///
/// The daemon watches for changes to alert definition files and automatically reloads
/// when changes are detected. You can also send SIGHUP to manually trigger a reload.
#[derive(Debug, Clone, Parser)]
pub struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	/// Path to Tamanu configuration file (config.json)
	///
	/// This file should contain database and email configuration.
	#[arg(long, env = "TAMANU_CONFIG")]
	pub config: PathBuf,

	/// Folder containing alert definitions
	///
	/// This folder will be read recursively for files with the `.yaml` or `.yml` extension.
	/// Can be provided multiple times.
	#[arg(long)]
	pub dir: Vec<PathBuf>,

	/// Don't actually send alerts, just print them to stdout
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

	debug!(?args.config, "reading Tamanu configuration");
	let config_content = std::fs::read_to_string(&args.config)
		.into_diagnostic()
		.map_err(|e| miette!("failed to read config file: {e}"))?;

	let tamanu_config = if args.config.extension().is_some_and(|ext| ext == "json") {
		bestool_alertd::Config::from_json(&config_content)
			.map_err(|e| miette!("failed to parse config.json: {e}"))?
	} else {
		bestool_alertd::Config::from_toml(&config_content)?
	};

	let daemon_config = bestool_alertd::DaemonConfig::new(args.dir, String::new())
		.with_dry_run(args.dry_run)
		.with_colours(args.logging.color.enabled());

	bestool_alertd::run(daemon_config, tamanu_config).await
}
