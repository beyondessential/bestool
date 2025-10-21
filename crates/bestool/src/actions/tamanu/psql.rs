use bestool_psql::highlighter::Theme;
use clap::Parser;
use miette::{Context as _, Result};

use crate::actions::Context;

use super::{TamanuArgs, config::load_config, find_tamanu};

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
	/// writes, either pass this flag, or call `\W` within the session.
	///
	/// This also disables autocommit, so you need to issue a COMMIT; command whenever you perform
	/// a write (insert, update, etc), as an extra safety measure.
	///
	/// Additionally, enabling write mode will prompt for an OTS value. This should be the name of
	/// a person supervising the write operation, or a short message describing why you don't need
	/// one, such as "demo" or "emergency".
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

	/// Disable schema-aware autocompletion.
	///
	/// By default, queries the database schema on startup to provide table/column completion.
	/// Use the `\refresh` command to manually refresh the schema cache during a session.
	/// This is not available during a transaction for safety reasons.
	#[arg(long)]
	pub disable_schema_completion: bool,

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

	/// Syntax highlighting theme (light, dark, or auto)
	///
	/// Controls the color scheme for SQL syntax highlighting in the input line.
	/// 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails.
	#[arg(long, default_value = "auto")]
	pub theme: Theme,

	/// Additional, arbitrary arguments to pass to `psql`
	///
	/// If it has dashes (like `--password pass`), you need to prefix this with two dashes:
	///
	/// bestool tamanu psql -- --password pass
	#[arg(trailing_var_arg = true)]
	pub args: Vec<String>,
}

pub async fn run(ctx: Context<TamanuArgs, PsqlArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;

	let config = load_config(&root, None)?;
	let name = &config.db.name;
	let (username, password) = if let Some(ref user) = ctx.args_sub.username {
		// First, check if this matches a report schema connection
		if let Some(ref report_schemas) = config.db.report_schemas {
			if let Some(connection) = report_schemas.connections.get(user)
				&& !connection.username.is_empty()
			{
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

	// Build psql arguments for the database connection
	let mut psql_args = vec![
		"--dbname".to_string(),
		name.to_string(),
		"--username".to_string(),
		username.to_string(),
	];

	if let Some(ref host) = config.db.host
		&& host != "localhost"
	{
		psql_args.push("--host".to_string());
		psql_args.push(host.to_string());
	}

	if let Some(port) = config.db.port {
		psql_args.push("--port".to_string());
		psql_args.push(port.to_string());
	}

	// Add any additional user-specified arguments
	psql_args.extend(ctx.args_sub.args.iter().cloned());

	// Set PGPASSWORD environment variable
	unsafe {
		std::env::set_var("PGPASSWORD", password);
	}

	// Create the bestool-psql config
	let psql_config = bestool_psql::PsqlConfig {
		program: ctx.args_sub.program,
		args: psql_args,
		write: ctx.args_sub.write,
		ots: None,             // Tamanu doesn't use OTS by default
		psqlrc: String::new(), // Use empty psqlrc, defaults will be set by bestool-psql
		passthrough: ctx.args_sub.passthrough,
		disable_schema_completion: ctx.args_sub.disable_schema_completion,
		history_path: bestool_psql::history::History::default_path()
			.ok()
			.unwrap_or_else(|| std::path::PathBuf::from(".bestool-psql-history.redb")),
		user: Some(username.to_string()),
		theme: ctx.args_sub.theme.resolve(),
	};

	bestool_psql::set_console_codepage(ctx.args_sub.codepage);

	// Run bestool-psql
	let exit_code = bestool_psql::run(psql_config).wrap_err("failed to execute psql")?;

	// Exit with the same code as psql
	if exit_code != 0 {
		std::process::exit(exit_code);
	}

	Ok(())
}
