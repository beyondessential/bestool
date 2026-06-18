use clap::Parser;
use miette::Result;

use bestool_tamanu::{
	config::{Database, database_url_override, load_config},
	connection_url::ConnectionUrlBuilder,
};

use crate::actions::{
	Context,
	tamanu::{TamanuArgs, find_tamanu},
};

/// Generate a DATABASE_URL connection string
///
/// This command reads the Tamanu configuration and outputs a PostgreSQL connection string
/// in the standard DATABASE_URL format: `postgresql://user:password@host/database`.
///
/// If the TAMANU_DATABASE_URL environment variable is set, it is used instead of
/// the config (and printed verbatim), so no Tamanu install is required.
///
/// Aliases: db, u, url
#[derive(Debug, Clone, Parser)]
pub struct DbUrlArgs {
	/// Database user to use in the connection string.
	///
	/// If the value matches one of the report schema connection names
	/// (e.g., "raw", "reporting"), credentials will be taken from that connection.
	#[arg(short = 'U', long)]
	pub username: Option<String>,
}

pub async fn run(args: DbUrlArgs, ctx: Context) -> Result<()> {
	// When TAMANU_DATABASE_URL is set it's authoritative, and no Tamanu install
	// is needed. With no `-U`, echo it verbatim so any Unix-socket / query-param
	// form survives untouched; with `-U`, re-point it at the requested role
	// (the override carries no report-schema credentials, so no password).
	if let Some(url) = database_url_override() {
		match args.username {
			None => println!("{url}"),
			Some(user) => {
				let db = Database::from_url(&url)?;
				let builder = ConnectionUrlBuilder {
					username: user,
					password: None,
					host: db.host.unwrap_or_else(|| "localhost".to_string()),
					port: db.port,
					database: db.name,
					ssl_mode: None,
				};
				println!("{}", builder.build());
			}
		}
		return Ok(());
	}

	let (_, root) = find_tamanu(ctx.require::<TamanuArgs>()).await?;
	let config = load_config(&root, None)?;

	let (username, password) = if let Some(ref user) = args.username {
		if let Some(ref report_schemas) = config.db.report_schemas {
			if let Some(connection) = report_schemas.connections.get(user)
				&& !connection.username.is_empty()
			{
				(
					connection.username.clone(),
					Some(connection.password.clone()),
				)
			} else if user == &config.db.username {
				(config.db.username.clone(), Some(config.db.password.clone()))
			} else {
				(user.clone(), None)
			}
		} else if user == &config.db.username {
			(config.db.username.clone(), Some(config.db.password.clone()))
		} else {
			(user.clone(), None)
		}
	} else {
		(config.db.username.clone(), Some(config.db.password.clone()))
	};

	let password = if password.as_ref().is_some_and(|p| p.is_empty()) {
		None
	} else {
		password
	};

	let builder = ConnectionUrlBuilder {
		username,
		password,
		host: config
			.db
			.host
			.clone()
			.unwrap_or_else(|| "localhost".to_string()),
		port: config.db.port,
		database: config.db.name.clone(),
		ssl_mode: None,
	};
	let url = builder.build();

	println!("{}", url);

	Ok(())
}
