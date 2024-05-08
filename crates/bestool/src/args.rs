use std::{env::var, fs::metadata, io::stderr, path::PathBuf};

use clap::{ArgAction, Parser, ValueEnum, ValueHint};
use miette::{bail, Result};
use tracing::{debug, warn};
use tracing_appender::{non_blocking, non_blocking::WorkerGuard, rolling};

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
	///
	/// You can also set the NO_COLOR environment variable to disable colours.
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
		default_value = "0",
	)]
	pub verbose: u8,

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

	/// Omit timestamps in logs
	///
	/// This can be useful when running under systemd, to avoid having two timestamps.
	///
	/// This option is ignored if the log file is set, or when using $RUST_LOG (as logging is
	/// initialized before arguments are parsed in that case).
	#[arg(long)]
	pub log_timeless: bool,

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

pub fn get_args() -> Result<(Args, Option<WorkerGuard>)> {
	let prearg_logs = logging_preargs();
	if prearg_logs {
		warn!("⚠ RUST_LOG environment variable set or hardcoded, logging options have no effect");
	}

	debug!("parsing arguments");
	let mut args = Args::parse();

	let log_guard = if !prearg_logs {
		Some(logging_postargs(&args)?)
	} else {
		None
	};

	// https://no-color.org/
	if var("NO_COLOR").is_ok() {
		debug!("NO_COLOR environment variable set, ignoring --color option");
		args.color = ColourMode::Never;
	} else if enable_ansi_support::enable_ansi_support().is_err() {
		debug!("failed to enable colour support on Windows, disabling colour");
		args.color = ColourMode::Never;
	}

	debug!(?args, "got arguments");
	Ok((args, log_guard))
}

pub fn logging_preargs() -> bool {
	let mut log_on = false;

	#[cfg(feature = "dev-console")]
	match console_subscriber::try_init() {
		Ok(_) => {
			warn!("dev-console enabled");
			log_on = true;
		}
		Err(e) => {
			eprintln!("Failed to initialise tokio console, falling back to normal logging\n{e}")
		}
	}

	if !log_on && var("RUST_LOG").is_ok() {
		match tracing_subscriber::fmt::try_init() {
			Ok(()) => {
				warn!(RUST_LOG=%var("RUST_LOG").unwrap(), "logging configured from RUST_LOG");
				log_on = true;
			}
			Err(e) => eprintln!("Failed to initialise logging with RUST_LOG, falling back\n{e}"),
		}
	}

	log_on
}

pub fn logging_postargs(args: &Args) -> Result<WorkerGuard> {
	let (log_writer, guard) = if let Some(file) = &args.log_file {
		let is_dir = metadata(&file).map_or(false, |info| info.is_dir());
		let (dir, filename) = if is_dir {
			(
				file.to_owned(),
				PathBuf::from(format!(
					"bestool.{}.log",
					chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ")
				)),
			)
		} else if let (Some(parent), Some(file_name)) = (file.parent(), file.file_name()) {
			(parent.into(), PathBuf::from(file_name))
		} else {
			bail!("Failed to determine log file name");
		};

		non_blocking(rolling::never(dir, filename))
	} else {
		non_blocking(stderr())
	};

	let mut builder = tracing_subscriber::fmt().with_env_filter(match args.verbose {
		0 => "info",
		1 => "info,bestool=debug",
		2 => "debug",
		3 => "debug,bestool=trace",
		_ => "trace",
	});

	match args.color {
		ColourMode::Never => {
			builder = builder.with_ansi(false);
		}
		ColourMode::Always => {
			builder = builder.with_ansi(true);
		}
		ColourMode::Auto => {}
	}

	if args.verbose > 0 {
		use tracing_subscriber::fmt::format::FmtSpan;
		builder = builder.with_span_events(FmtSpan::NEW | FmtSpan::CLOSE);
	}

	match if args.log_file.is_some() {
		builder.json().with_writer(log_writer).try_init()
	} else if args.verbose > 3 {
		if args.log_timeless {
			builder.without_time().with_writer(log_writer).try_init()
		} else {
			builder.pretty().with_writer(log_writer).try_init()
		}
	} else {
		if args.log_timeless {
			builder.without_time().with_writer(log_writer).try_init()
		} else {
			builder.with_writer(log_writer).try_init()
		}
	} {
		Ok(()) => debug!("logging initialised"),
		Err(e) => eprintln!("Failed to initialise logging, continuing with none\n{e}"),
	}

	Ok(guard)
}

#[test]
fn verify_cli() {
	use clap::CommandFactory;
	Args::command().debug_assert()
}
