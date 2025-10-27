use bestool_psql2::highlighter::Theme;
use bestool_psql2::history::History;
use bestool_psql2::ots;
use bestool_psql2::PsqlConfig;
use clap::Parser;
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{miette, Result};
use std::path::PathBuf;
use tracing::debug;

/// Async PostgreSQL client
#[derive(Debug, Clone, Parser)]
pub struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	/// Database name or connection string
	///
	/// Can be a simple database name (e.g., 'mydb') or full connection string
	/// (e.g., 'postgresql://user:password@localhost:5432/dbname')
	pub dbname: String,

	/// Database user (for tracking, defaults to $USER)
	#[arg(short = 'U', long)]
	pub user: Option<String>,

	/// Enable write mode for this session
	///
	/// By default the session is read-only. To enable writes, pass this flag.
	/// This also disables autocommit, so you need to issue a COMMIT; command
	/// whenever you perform a write (insert, update, etc), as an extra safety measure.
	#[arg(short = 'W', long)]
	pub write: bool,

	/// Syntax highlighting theme (light, dark, or auto)
	///
	/// Controls the color scheme for SQL syntax highlighting in the input line.
	/// 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails.
	#[arg(long, default_value = "auto")]
	pub theme: Theme,

	/// Path to history database (default: ~/.local/state/bestool-psql/history.redb)
	#[arg(long)]
	pub history_path: Option<PathBuf>,
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

	let theme = args.theme.resolve();
	debug!(?theme, "using syntax highlighting theme");

	let connection_string = if args.dbname.contains("://") {
		args.dbname
	} else {
		format!("postgresql://localhost/{}", args.dbname)
	};

	debug!(?connection_string, "using connection string");

	let history_path = if let Some(path) = args.history_path {
		path
	} else {
		History::default_path()?
	};

	let ots = if args.write {
		Some(ots::prompt_for_ots(&history_path)?)
	} else {
		None
	};

	let config = PsqlConfig {
		connection_string,
		user: args.user,
		theme,
		history_path,
		database_name: String::new(), // Will be queried from database
		write: args.write,
		ots,
	};

	if args.write {
		eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");
	}

	bestool_psql2::run(config).await
}
