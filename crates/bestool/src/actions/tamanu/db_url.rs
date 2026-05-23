use clap::Parser;
use miette::Result;

use bestool_tamanu::{config::load_config, connection_url::ConnectionUrlBuilder};

use crate::actions::{
	Context,
	tamanu::{TamanuArgs, find_tamanu},
};

/// Generate a DATABASE_URL connection string
///
/// This command reads the Tamanu configuration and outputs a PostgreSQL connection string
/// in the standard DATABASE_URL format: `postgresql://user:password@host/database`.
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
	let (_, root) = find_tamanu(ctx.require::<TamanuArgs>())?;
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
