use std::io::Write;

use clap::Parser;
use miette::{Context as _, IntoDiagnostic, Result};

use crate::actions::Context;

use super::{TamanuArgs, config::load_config, find_postgres_bin, find_tamanu};

/// Connect to Tamanu's db via `psql`.
///
/// Aliases: p, pg, sql
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

	/// Set the console codepage (Windows-only)
	#[arg(long, default_value = "65001")]
	pub codepage: u32,
}

pub async fn run(ctx: Context<TamanuArgs, PsqlArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;

	let config = load_config(&root, None)?;
	let name = &config.db.name;
	let (username, password) = if let Some(ref user) = ctx.args_sub.username {
		// First, check if this matches a report schema connection
		if let Some(ref report_schemas) = config.db.report_schemas {
			if let Some(connection) = report_schemas.connections.get(user) && !connection.username.is_empty() {
				(connection.username.as_str(), connection.password.as_str())
			} else if user == &config.db.username {
				// User matches main db user
				(config.db.username.as_str(), config.db.password.as_str())
			} else {
				// User doesn't match anything, rely on psql password prompt
				(user.as_str(), "")
			}
		} else if user == &config.db.username {
			// No report schemas, check if matches main user
			(config.db.username.as_str(), config.db.password.as_str())
		} else {
			// User doesn't match, rely on psql password prompt
			(user.as_str(), "")
		}
	} else {
		// No user specified, use main db credentials
		(config.db.username.as_str(), config.db.password.as_str())
	};

	// Set the console encoding to UTF-8
	#[cfg(windows)]
	unsafe {
		windows::Win32::System::Console::SetConsoleCP(ctx.args_top.codepage).into_diagnostic()?
	}

	let mut rc = tempfile::Builder::new().tempfile().into_diagnostic()?;
	write!(
		rc.as_file_mut(),
		"\\encoding UTF8\n\\timing\n{ro}",
		ro = if ctx.args_sub.write {
			""
		} else {
			"SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY;"
		},
	)
	.into_diagnostic()?;

	let psql_path = find_postgres_bin(&ctx.args_sub.program)?;

	let mut args = vec!["--dbname", name, "--username", username];

	if let Some(ref host) = config.db.host
		&& host != "localhost"
	{
		args.push("--host");
		args.push(host);
	}

	let port_string;
	if let Some(port) = config.db.port {
		port_string = port.to_string();
		args.push("--port");
		args.push(&port_string);
	}

	if ctx.args_sub.write && ctx.args_sub.program == "psql" {
		args.push("--set=AUTOCOMMIT=OFF");
		eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");
	}
	args.extend(ctx.args_sub.args.iter().map(|s| s.as_str()));

	duct::cmd(psql_path, &args)
		.env("PSQLRC", rc.path())
		.env("PGPASSWORD", password)
		.run()
		.into_diagnostic()
		.wrap_err("failed to execute psql")?;

	Ok(())
}
