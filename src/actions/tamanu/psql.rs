use clap::Parser;
use miette::{miette, IntoDiagnostic, Result};

use crate::actions::Context;

use super::config::{merge_json, package_config};
use super::{find_tamanu, TamanuArgs};

/// Connect to Tamanu's db via `psql`.
#[derive(Debug, Clone, Parser)]
pub struct PsqlArgs {
	/// Package to look at
	#[arg(short, long)]
	pub package: String,

	/// Include defaults
	#[arg(short = 'D', long)]
	pub defaults: bool,
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
	fn try_get_string_key<'a>(db: &'a tera::Value, key: &str) -> Result<&'a str> {
		db
			.get(key)
			.and_then(|u| u.as_str())
			.ok_or_else(|| miette!("key 'db.{key}' not found or string"))
	}
	let name = try_get_string_key(db, "name")?;
	let username = try_get_string_key(db, "username")?;
	let password = try_get_string_key(db, "password")?;

	duct::cmd!(
		"psql",
		"--host",
		"localhost",
		"--dbname",
		name,
		"--username",
		username,
	)
	.env(
		"PGPASSWORD",
		password,
	)
	.env("PSQL_HISTORY", root.with_file_name("psql.history"))
	.run()
	.into_diagnostic()?;

	Ok(())
}
