use clap::Parser;
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{Result, miette};
use tracing::debug;

/// BES Tooling
#[derive(Debug, Clone, Parser)]
#[command(
	author,
	version,
	after_help = "Want more detail? Try the long '--help' flag!",
	after_long_help = "Didn't expect this much output? Use the short '-h' flag to get short help."
)]
pub struct Args {
	#[command(flatten)]
	pub logging: LoggingArgs,

	/// What to do
	#[command(subcommand)]
	pub action: crate::actions::Action,
}

pub fn get_args() -> Result<(Args, WorkerGuard)> {
	// Dynamic shell completions: when invoked with the `COMPLETE` env var set
	// (via the registration snippet `source <(COMPLETE=bash bestool)` etc.),
	// this emits completions and exits. Must run before anything writes to
	// stdout, hence first thing here.
	#[cfg(feature = "completions")]
	{
		use clap::CommandFactory as _;
		clap_complete::CompleteEnv::with_factory(Args::command).complete();
	}

	let log_guard = PreArgs::parse().setup().map_err(|err| miette!("{err}"))?;

	debug!("parsing arguments");
	let args = Args::parse();

	let log_guard = match log_guard {
		Some(g) => g,
		None => args
			.logging
			.setup(|v| match v {
				0 => "warn,bestool=info,bestool_psql=info,algae_cli=info",
				1 => "info,bestool=debug",
				2 => "debug",
				3 => "debug,bestool=trace",
				_ => "trace",
			})
			.map_err(|err| miette!("{err}"))?,
	};

	debug!(?args, "got arguments");
	Ok((args, log_guard))
}

#[test]
fn verify_cli() {
	use clap::CommandFactory;
	Args::command().debug_assert()
}
