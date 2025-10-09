use clap::Parser;
use miette::Result;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};

use crate::actions::{
	tamanu::{config::load_config, find_tamanu, TamanuArgs},
	Context,
};

/// Characters to encode in userinfo (username:password) part of URL
const USERINFO_ENCODE_SET: &AsciiSet = &CONTROLS
	.add(b':')
	.add(b'@')
	.add(b'/')
	.add(b'?')
	.add(b'#')
	.add(b'[')
	.add(b']')
	.add(b'$');

/// Generate a DATABASE_URL connection string from Tamanu config.
///
/// This command reads the Tamanu configuration and outputs a PostgreSQL connection string
/// in the standard DATABASE_URL format: `postgresql://user:password@host/database`.
#[derive(Debug, Clone, Parser)]
pub struct DburlArgs {
	/// Database user to use in the connection string.
	///
	/// If the value matches one of the report schema connection names
	/// (e.g., "raw", "reporting"), credentials will be taken from that connection.
	#[arg(short = 'U', long)]
	pub username: Option<String>,
}

pub async fn run(ctx: Context<TamanuArgs, DburlArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;
	let config = load_config(&root, None)?;

	let (username, password) = if let Some(ref user) = ctx.args_sub.username {
		if let Some(ref report_schemas) = config.db.report_schemas {
			if let Some(connection) = report_schemas.connections.get(user) {
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

	let host = config.db.host.as_deref().unwrap_or("localhost");
	let database = &config.db.name;

	let host_with_port = if let Some(port) = config.db.port {
		format!("{}:{}", host, port)
	} else {
		host.to_string()
	};

	let encoded_username = utf8_percent_encode(&username, USERINFO_ENCODE_SET);
	let url = if let Some(password) = password {
		let encoded_password = utf8_percent_encode(&password, USERINFO_ENCODE_SET);
		format!(
			"postgresql://{}:{}@{}/{}",
			encoded_username, encoded_password, host_with_port, database
		)
	} else {
		format!(
			"postgresql://{}@{}/{}",
			encoded_username, host_with_port, database
		)
	};

	println!("{}", url);

	Ok(())
}
