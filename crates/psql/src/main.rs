use bestool_psql::history::History;
use clap::Parser;
use miette::{IntoDiagnostic, Result};
use std::{fs, path::PathBuf};

/// Interactive psql wrapper with custom readline and editor interception
#[derive(Debug, Clone, Parser)]
#[command(name = "bestool-psql")]
#[command(about = "Connect to PostgreSQL via psql with enhanced features")]
pub struct Args {
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

	/// Set the console codepage (Windows-only, ignored on other platforms)
	#[arg(long, default_value = "65001")]
	pub codepage: u32,

	/// Path to psql executable
	#[arg(long, default_value = "psql")]
	pub psql_path: PathBuf,

	/// Do not read the startup file (~/.psqlrc)
	#[arg(short = 'X', long)]
	pub no_psqlrc: bool,

	/// Path to history database (default: ~/.cache/bestool-psql/history.redb)
	#[arg(long)]
	pub history_path: Option<PathBuf>,

	/// Disable history recording
	#[arg(long)]
	pub no_history: bool,

	/// Database user (for history tracking, defaults to $USER)
	#[arg(short = 'U', long)]
	pub user: Option<String>,

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

fn main() -> Result<()> {
	let args = Args::parse();

	// Set the console encoding to UTF-8 on Windows
	#[cfg(windows)]
	unsafe {
		use std::os::windows::io::AsRawHandle;
		use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
		SetConsoleCP(args.codepage);
		SetConsoleOutputCP(args.codepage);
	}

	// Read .psqlrc unless --no-psqlrc is specified
	let psqlrc = if args.no_psqlrc {
		String::new()
	} else {
		read_psqlrc()?
	};

	// Determine history path unless --no-history is specified
	let history_path = if args.no_history {
		None
	} else if let Some(path) = args.history_path {
		Some(path)
	} else {
		History::default_path().ok()
	};

	let config = bestool_psql::PsqlConfig {
		psql_path: args.psql_path,
		write: args.write,
		args: args.args,
		psqlrc,
		history_path,
		user: args.user,
	};

	if args.write {
		eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");
	}

	std::process::exit(bestool_psql::run(config)?);
}
