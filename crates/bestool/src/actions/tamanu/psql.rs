use std::io::Write;

use clap::Parser;
use miette::{Context as _, IntoDiagnostic, Result};

use crate::actions::Context;

use super::{config::load_config, find_postgres_bin, find_tamanu, TamanuArgs};

/// Connect to Tamanu's db via `psql`.
#[cfg_attr(docsrs, doc("\n\n**Command**: `bestool tamanu psql`"))]
#[derive(Debug, Clone, Parser)]
pub struct PsqlArgs {
	/// Connect to postgres with a different username.
	///
	/// This may prompt for a password depending on your local settings and pg_hba config.
	#[arg(short = 'U', long)]
	pub username: Option<String>,

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

	/// Additional, arbitrary arguments to pass to `psql`
	///
	/// If it has dashes (like `--password pass`), you need to prefix this with two dashes:
	///
	/// bestool tamanu psql -- --password pass
	#[arg(trailing_var_arg = true)]
	pub args: Vec<String>,

	/// Alternative postgres program to invoke
	///
	/// Advanced! You can swap out psql for another postgres program. This will be passed options
	/// derived from the config (database credentials) so may not work if those aren't expected.
	#[arg(long, default_value = "psql")]
	pub program: String,
}

pub async fn run(ctx: Context<TamanuArgs, PsqlArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;

	let config = load_config(&root, None)?;
	let name = &config.db.name;
	let (username, password) = if let Some(ref username) = ctx.args_sub.username {
		// Rely on `psql` password prompt by making the password parameter empty.
		(username.as_str(), "")
	} else {
		(config.db.username.as_str(), config.db.password.as_str())
	};

	// By default, consoles on Windows use a different codepage from other parts of the system.
	// What that implies for us is not clear, but this code is here just in case.
	// See https://www.postgresql.org/docs/current/app-psql.html
	#[cfg(windows)]
	unsafe {
		windows::Win32::System::Console::SetConsoleCP(1252).into_diagnostic()?
	}

	let mut rc = tempfile::Builder::new().tempfile().into_diagnostic()?;
	write!(
		rc.as_file_mut(),
		"{ro}",
		ro = if ctx.args_sub.write {
			""
		} else {
			"SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY;"
		},
	)
	.into_diagnostic()?;

	let psql_path = find_postgres_bin(&ctx.args_sub.program)?;

	let mut args = vec!["--dbname", name, "--username", username];
	if ctx.args_sub.write && ctx.args_sub.program == "psql" {
		args.push("--set=AUTOCOMMIT=OFF");
		eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");
	}
	args.extend(ctx.args_sub.args.iter().map(|s| s.as_str()));

	// Use the default host, which is the localhost via Unix-domain socket on Unix or TCP/IP on Windows
	duct::cmd(psql_path, &args)
		.env("PSQLRC", rc.path())
		.env("PGPASSWORD", password)
		.run()
		.into_diagnostic()
		.wrap_err("failed to execute psql")?;

	Ok(())
}
