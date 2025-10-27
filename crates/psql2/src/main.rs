use bestool_psql2::PsqlConfig;
use clap::Parser;
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{miette, Result};
use tracing::debug;

/// Async PostgreSQL client
#[derive(Debug, Clone, Parser)]
pub struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	/// Database user (for tracking, defaults to $USER)
	#[arg(short = 'U', long)]
	pub user: Option<String>,

	/// Database connection string
	///
	/// Example: postgresql://user:password@localhost:5432/dbname
	#[arg(short = 'd', long)]
	pub connection: String,
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
				0 => "info",
				1 => "info,bestool_psql2=debug",
				2 => "debug",
				3 => "debug,bestool_psql2=trace",
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

	debug!("starting psql2");

	let config = PsqlConfig {
		connection_string: args.connection,
		user: args.user,
	};

	bestool_psql2::run(config).await
}
