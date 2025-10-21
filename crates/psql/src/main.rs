use bestool_psql::highlighter::Theme;
use bestool_psql::history::History;
use clap::Parser;
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{miette, IntoDiagnostic, Result};
use std::{fs, path::PathBuf};
use tracing::debug;

/// Interactive PostgreSQL terminal
///
/// Custom commands:
///
///   \W        - Toggle write mode (switches between read-only and read-write sessions)
///
///   \refresh  - Reload schema cache (refreshes table/column/function autocompletion)
///
/// For psql help and options, see `psql --help`. To pass those into this tool, add `--` and then the options.
#[derive(Debug, Clone, Parser)]
pub struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	/// Enable write mode for this psql.
	///
	/// By default we set `TRANSACTION READ ONLY` for the session, which prevents writes. To enable
	/// writes, either pass this flag, or call `SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE;`
	/// within the session.
	///
	/// This also disables autocommit, so you need to issue a COMMIT; command whenever you perform
	/// a write (insert, update, etc), as an extra safety measure.
	#[arg(short = 'W', long)]
	pub write: bool,

	/// Launch psql directly without wrapper (passthrough mode).
	///
	/// This mode runs native psql with its own readline, which means you can use psql's native
	/// tab completion on unix but lose bestool features like audit logging and custom commands.
	///
	/// Enforces read-only mode for safety.
	#[arg(long, conflicts_with = "write")]
	pub passthrough: bool,

	/// Enable schema-aware autocompletion.
	///
	/// Disabled by default because it can be a bit buggy and slow.
	///
	/// Queries the database schema on startup to provide table/column completion.
	/// Use the `\refresh` command to manually refresh the schema cache during a session.
	/// This is not available during a transaction for safety reasons.
	#[arg(long)]
	pub enable_schema_completion: bool,

	/// Set the console codepage (Windows-only, ignored on other platforms)
	#[arg(long, default_value = "65001")]
	pub codepage: u32,

	/// Alternative postgres program to invoke
	///
	/// Advanced! You can swap out psql for another postgres program. This will be passed options
	/// derived from the config (database credentials) so may not work if those aren't expected.
	///
	/// If the path is absolute, it will be used directly. Otherwise, it will be searched for in
	/// the PATH or in the PostgreSQL installation directory.
	#[arg(long, default_value = "psql")]
	pub program: String,

	/// Do not read the startup file (~/.psqlrc)
	#[arg(short = 'X', long)]
	pub no_psqlrc: bool,

	/// Path to history database (default: ~/.local/state/bestool-psql/history.redb)
	#[arg(long)]
	pub history_path: Option<PathBuf>,

	/// Database user (for history tracking, defaults to $USER)
	#[arg(short = 'U', long)]
	pub user: Option<String>,

	/// Syntax highlighting theme (light, dark, or auto)
	///
	/// Controls the color scheme for SQL syntax highlighting in the input line.
	/// 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails.
	#[arg(long, default_value = "auto")]
	pub theme: Theme,

	/// Arbitrary arguments to pass to `psql`; prefix with `--`
	///
	/// bestool-psql -- --password pass
	#[arg(trailing_var_arg = true)]
	pub args: Vec<String>,
}

fn read_psqlrc() -> Result<String> {
	let psqlrc_path = if let Some(home) = std::env::var_os("HOME") {
		PathBuf::from(home).join(".psqlrc")
	} else if let Some(userprofile) = std::env::var_os("USERPROFILE") {
		// Windows fallback
		PathBuf::from(userprofile).join(".psqlrc")
	} else {
		return Ok(String::new());
	};

	if psqlrc_path.exists() {
		fs::read_to_string(&psqlrc_path)
			.into_diagnostic()
			.or_else(|_| Ok(String::new()))
	} else {
		Ok(String::new())
	}
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

	bestool_psql::set_console_codepage(args.codepage);

	let history_path = if let Some(path) = args.history_path.clone() {
		path
	} else {
		History::default_path()?
	};

	// Prompt for OTS when write mode is enabled
	let ots = if args.write {
		Some(bestool_psql::prompt_for_ots(&history_path)?)
	} else {
		None
	};

	let psqlrc = if args.no_psqlrc {
		String::new()
	} else {
		read_psqlrc()?
	};

	let theme = args.theme.resolve();
	debug!(?theme, "using syntax highlighting theme");

	let config = bestool_psql::PsqlConfig {
		program: args.program,
		write: args.write,
		args: args.args,
		psqlrc,
		history_path,
		user: args.user,
		ots,
		passthrough: args.passthrough,
		disable_schema_cache: !args.enable_schema_completion,
		theme,
	};

	if args.write {
		eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");
	}

	debug!(?config, "starting psql");

	std::process::exit(bestool_psql::run(config)?);
}
