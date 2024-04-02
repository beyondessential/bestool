use std::fs;

use clap::Parser;
use miette::{miette, IntoDiagnostic, Result};

use super::config::{merge_json, package_config};
use super::{find_tamanu, TamanuArgs};
use crate::actions::Context;

use tracing::{debug, instrument};

/// Connect to Tamanu's db via `psql`.
#[derive(Debug, Clone, Parser)]
pub struct PsqlArgs {
	/// Package to look at
	#[arg(short, long)]
	pub package: String,

	/// Include defaults
	#[arg(short = 'D', long)]
	pub defaults: bool,

	/// If given, this overwrites the username in the config and prompts for a password
	#[arg(short, long)]
	pub username: Option<String>,
}

pub async fn run(ctx: Context<TamanuArgs, PsqlArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;

	let config = if ctx.args_sub.defaults {
		merge_json(
			package_config(&root, &ctx.args_sub.package, "default.json5")?,
			package_config(&root, &ctx.args_sub.package, "local.json5")?,
		)
	} else {
		package_config(&root, &ctx.args_sub.package, "local.json5")?
	};

	let db = config
		.get("db")
		.ok_or_else(|| miette!("key 'db' not found"))?;
	let name = try_get_string_key(db, "name")?;
	let (username, password) = if let Some(ref username) = ctx.args_sub.username {
		// Rely on `psql` password prompt by making the password parameter empty.
		(username.as_str(), "")
	} else {
		(
			try_get_string_key(db, "username")?,
			try_get_string_key(db, "password")?,
		)
	};

	// By default, consoles on Windows use a different codepage from other parts of the system.
	// What that implies for us is not clear, but this code is here just in case.
	// See https://www.postgresql.org/docs/current/app-psql.html
	#[cfg(windows)]
	unsafe {
		windows::Win32::System::Console::SetConsoleCP(1252).into_diagnostic()?
	}

	// Use the default host, which is the localhost via Unix-domain socket on Unix or TCP/IP on Windows
	duct::cmd!(find_psql()?, "--dbname", name, "--username", username,)
		.env("PGPASSWORD", password)
		.env("PSQL_HISTORY", root.with_file_name("psql.history"))
		.run()
		.into_diagnostic()?;

	Ok(())
}

fn try_get_string_key<'a>(db: &'a tera::Value, key: &str) -> Result<&'a str> {
	db.get(key)
		.and_then(|u| u.as_str())
		.ok_or_else(|| miette!("key 'db.{key}' not found or string"))
}

#[instrument(level = "debug")]
fn find_psql() -> Result<String> {
	// On Windows, find `psql` assuming the standard instllation using the instller
	// because PATH on Windows is not reliable.
	let root = "C:\\Program Files\\PostgreSQL";
	if cfg!(windows) {
		let version = fs::read_dir(root)
			.into_diagnostic()?
			.inspect(|res| debug!(?res, "reading PostgreSQL installation"))
			.filter_map(|res| {
				res.map(|dir| {
					dir.file_name()
						.into_string()
						.ok()
						.and_then(|name| name.parse::<u32>().ok())
				})
				.transpose()
			})
			// Use `u32::MAX` in case of `Err` so that we always catch IO errors.
			.max_by_key(|res| res.as_ref().cloned().unwrap_or(u32::MAX))
			.ok_or_else(|| miette!("the Postgres root {root} is empty"))?
			.into_diagnostic()?;

		Ok(format!("{root}\\{version}\\bin\\psql.exe"))
	} else {
		Ok("psql".to_string())
	}
}
