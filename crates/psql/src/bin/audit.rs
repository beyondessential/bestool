use bestool_psql::{AuditArgs, run_audit_cli};
use clap::Parser;
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{Result, miette};
use tracing::debug;

/// Read and maintain the bestool-psql audit log.
#[derive(Debug, Clone, Parser)]
pub struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	#[command(flatten)]
	audit: AuditArgs,
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
				0 => "bestool_psql=info",
				1 => "info,bestool_psql=debug",
				2 => "debug",
				3 => "debug,bestool_psql=trace",
				_ => "trace",
			})
			.map_err(|err| miette!("{err}"))?,
	};

	debug!(?args, "got arguments");
	Ok((args, log_guard))
}

fn main() -> Result<()> {
	let (args, _guard) = get_args()?;

	if run_audit_cli(args.audit)? {
		Ok(())
	} else {
		// A chain that does not hold is a non-zero exit, not an error to report
		// twice: verify has already said where and how.
		std::process::exit(1);
	}
}
