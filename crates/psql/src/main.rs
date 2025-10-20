use clap::Parser;
use miette::{IntoDiagnostic, Result};
use std::{io::Write, path::PathBuf};

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

	/// Arbitrary arguments to pass to `psql`; prefix with `--`
	///
	/// bestool-psql -- --password pass
	#[arg(trailing_var_arg = true)]
	pub args: Vec<String>,
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

	let config = bestool_psql::PsqlConfig {
		psql_path: args.psql_path,
		write: args.write,
		args: args.args,
		psqlrc: String::new(),
	};

	if args.write {
		eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");
	}

	std::process::exit(bestool_psql::run(config)?);
}
