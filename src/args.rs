use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum, ValueHint};
use tracing::{debug, warn};

/// BES Tooling
#[derive(Debug, Clone, Parser)]
#[command(
	author,
	version,
	long_version = format!("{} built from branch={} commit={} dirty={} source_timestamp={}",
		env!("CARGO_PKG_VERSION"),
		env!("GIT_BRANCH"),
		env!("GIT_COMMIT"),
		env!("GIT_DIRTY"),
		env!("SOURCE_TIMESTAMP"),
    ),
	after_help = "Want more detail? Try the long '--help' flag!",
	after_long_help = "Didn't expect this much output? Use the short '-h' flag to get short help.",
)]
#[cfg_attr(debug_assertions, command(before_help = "⚠ DEBUG BUILD ⚠"))]
pub struct Args {
	/// When to use terminal colours
	#[arg(long, default_value = "auto", value_name = "MODE", alias = "colour")]
	pub color: ColourMode,

	/// Set diagnostic log level
	///
	/// This enables diagnostic logging, which is useful for investigating bugs. Use multiple
	/// times to increase verbosity. Goes up to '-vvvvv'.
	///
	/// You may want to use with '--log-file' to avoid polluting your terminal.
	///
	/// Setting $RUST_LOG also works, and takes precedence, but is not recommended unless you know
	/// what you're doing. However, using $RUST_LOG is the only way to get logs from before these
	/// options are parsed.
	#[arg(
		long,
		short,
		action = ArgAction::Count,
		num_args = 0,
	)]
	pub verbose: Option<u8>,

	/// Write diagnostic logs to a file
	///
	/// This writes diagnostic logs to a file, instead of the terminal, in JSON format. If a log
	/// level was not already specified, this will set it to '-vvv'.
	///
	/// If the path provided is a directory, a file will be created in that directory. The file name
	/// will be the current date and time, in the format 'bestool.YYYY-MM-DDTHH-MM-SSZ.log'.
	#[arg(
		long,
		num_args = 0..=1,
		default_missing_value = ".",
		value_hint = ValueHint::AnyPath,
		value_name = "PATH",
	)]
	pub log_file: Option<PathBuf>,

	/// What to do
	#[command(subcommand)]
	pub action: crate::actions::Action,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ColourMode {
	Auto,
	Always,
	Never,
}

pub fn get_args() -> Args {
	if std::env::var("RUST_LOG").is_ok() {
		warn!("⚠ RUST_LOG environment variable set, logging options have no effect");
	}

	debug!("parsing arguments");
	let args = Args::parse();

	debug!(?args, "got arguments");
	args
}

#[test]
fn verify_cli() {
	use clap::CommandFactory;
	Args::command().debug_assert()
}
