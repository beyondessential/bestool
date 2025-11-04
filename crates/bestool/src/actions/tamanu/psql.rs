use std::path::PathBuf;

use clap::Parser;
use miette::Result;
use tracing::debug;

use crate::actions::Context;

use super::{TamanuArgs, config::load_config, find_tamanu};

/// Connect to Tamanu's database.
///
/// Aliases: p, pg, sql
#[derive(Debug, Clone, Parser)]
pub struct PsqlArgs {
	/// Connect to postgres with a different username.
	///
	/// This may prompt for a password depending on your local settings and pg_hba config.
	#[arg(short = 'U', long)]
	pub username: Option<String>,

	/// Connect to postgres with a connection URL.
	///
	/// This bypasses the discovery of credentials from Tamanu.
	#[arg(conflicts_with = "username")]
	pub url: Option<String>,

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

	/// Syntax highlighting theme (light, dark, or auto)
	///
	/// Controls the color scheme for SQL syntax highlighting in the input line.
	/// 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails.
	#[arg(long, default_value = "auto")]
	pub theme: bestool_psql::Theme,

	/// Path to audit database directory
	#[arg(long, value_name = "PATH", help = help_audit_path())]
	pub audit_path: Option<PathBuf>,
}

fn help_audit_path() -> String {
	format!(
		"Path to audit database directory (default: {})",
		bestool_psql::default_audit_dir()
	)
}

pub async fn run(ctx: Context<TamanuArgs, PsqlArgs>) -> Result<()> {
	let PsqlArgs {
		username,
		url,
		write,
		theme,
		audit_path,
	} = ctx.args_sub;

	let url = if let Some(url) = url {
		url
	} else {
		let (_, root) = find_tamanu(&ctx.args_top)?;
		let config = load_config(&root, None)?;
		let name = &config.db.name;
		let (username, password) = if let Some(ref user) = username {
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

		let host = config.db.host.as_deref().unwrap_or("localhost");
		let port = config.db.port.unwrap_or(5432);
		format!("postgresql://{username}:{password}@{host}:{port}/{name}")
	};

	debug!(url, "creating connection pool");
	let pool = bestool_psql::create_pool(&url).await?;

	// Install a Ctrl-C handler that sets a flag for query cancellation
	bestool_psql::register_sigint_handler()?;
	bestool_psql::run(bestool_psql::Config {
		pool,
		theme: theme.resolve(),
		audit_path,
		write,
		use_colours: ctx.args_top.use_colours,
	})
	.await
}
