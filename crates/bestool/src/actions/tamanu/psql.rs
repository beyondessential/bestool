use std::{collections::HashSet, path::PathBuf};

use clap::{Parser, ValueEnum};
use miette::{IntoDiagnostic as _, Result};
use tracing::debug;

use crate::actions::Context;

use super::{TamanuArgs, config::load_config, connection_url::ConnectionUrlBuilder, find_tamanu};

/// SSL mode for PostgreSQL connections
#[derive(Debug, Default, Clone, Copy, ValueEnum)]
pub enum SslMode {
	/// Disable SSL/TLS encryption
	Disable,
	/// Prefer SSL/TLS but allow unencrypted connections
	#[default]
	Prefer,
	/// Require SSL/TLS encryption
	Require,
}

impl SslMode {
	fn as_str(self) -> &'static str {
		match self {
			SslMode::Disable => "disable",
			SslMode::Prefer => "prefer",
			SslMode::Require => "require",
		}
	}
}

/// Connect to Tamanu's database.
///
/// Aliases: p, pg, sql
#[derive(Debug, Clone, Parser)]
pub struct PsqlArgs {
	/// Connect to postgres with a different username.
	///
	/// This may prompt for a password depending on your local settings and pg_hba config.
	#[arg(short = 'U', long, conflicts_with = "url")]
	pub username: Option<String>,

	/// SSL mode for the connection.
	///
	/// Defaults to 'prefer' which attempts SSL but falls back to non-SSL.
	/// Use 'disable' to skip SSL entirely (useful on Windows with certificate issues).
	/// Use 'require' to enforce SSL connections.
	///
	/// Ignored if a database URL is provided and it contains an sslmode parameter.
	#[arg(long, value_enum, default_value_t = SslMode::default())]
	pub ssl: SslMode,

	/// Connect to postgres with a connection URL.
	///
	/// This bypasses the discovery of credentials from Tamanu.
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
		ssl,
		url,
		write,
		theme,
		audit_path,
	} = ctx.args_sub;

	let url = if let Some(url) = url {
		let mut url = reqwest::Url::parse(&url).into_diagnostic()?;
		if !url.query_pairs().any(|(key, _)| key == "sslmode") {
			url.query_pairs_mut().append_pair("sslmode", ssl.as_str());
		}
		url.to_string()
	} else {
		let (_, root) = find_tamanu(&ctx.args_top)?;
		let config = load_config(&root, None)?;

		let (username, password) = if let Some(ref user) = username {
			// First, check if this matches a report schema connection
			if let Some(ref report_schemas) = config.db.report_schemas {
				if let Some(connection) = report_schemas.connections.get(user)
					&& !connection.username.is_empty()
				{
					(
						Some(connection.username.clone()),
						Some(connection.password.clone()),
					)
				} else if user == &config.db.username {
					// User matches main db user
					(
						Some(config.db.username.clone()),
						Some(config.db.password.clone()),
					)
				} else {
					// User doesn't match anything, rely on psql password prompt
					(Some(user.clone()), None)
				}
			} else if user == &config.db.username {
				// No report schemas, check if matches main user
				(
					Some(config.db.username.clone()),
					Some(config.db.password.clone()),
				)
			} else {
				// User doesn't match, rely on psql password prompt
				(Some(user.clone()), None)
			}
		} else {
			// No user specified, use main db credentials
			(
				Some(config.db.username.clone()),
				Some(config.db.password.clone()),
			)
		};

		let username = username.unwrap_or_else(|| config.db.username.clone());
		let password = if password.as_ref().is_some_and(|p| p.is_empty()) {
			None
		} else {
			password
		};

		let builder = ConnectionUrlBuilder {
			username,
			password,
			host: config.db.host.clone().unwrap_or_default(),
			port: config.db.port,
			database: config.db.name.clone(),
			ssl_mode: Some(ssl.as_str().to_string()),
		};
		builder.build()
	};

	debug!(url, "creating connection pool");
	let pool = bestool_psql::create_pool(&url).await?;

	// Install a Ctrl-C handler that sets a flag for query cancellation
	bestool_psql::register_sigint_handler()?;
	// Hardcode redaction for testing
	let mut redactions = HashSet::new();
	redactions.insert(bestool_psql::column_extractor::ColumnRef {
		schema: "public".to_string(),
		table: "local_system_facts".to_string(),
		column: "value".to_string(),
	});

	bestool_psql::run(bestool_psql::Config {
		pool,
		theme: theme.resolve(),
		audit_path,
		write,
		use_colours: ctx.args_top.use_colours,
		redact_mode: true,
		redactions,
	})
	.await
}
