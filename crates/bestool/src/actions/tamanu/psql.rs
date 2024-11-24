use std::{io::Write, str::FromStr};

use clap::Parser;
use miette::{Context as _, IntoDiagnostic, Result};
use tracing::info;

use crate::actions::Context;

use super::{
	config::{merge_json, package_config},
	find_package, find_postgres_bin, find_tamanu, ApiServerKind, TamanuArgs,
};

/// Connect to Tamanu's db via `psql`.
#[cfg_attr(docsrs, doc("\n\n**Command**: `bestool tamanu psql`"))]
#[derive(Debug, Clone, Parser)]
pub struct PsqlArgs {
	/// Package to load config from.
	///
	/// By default, this command looks for the most recent installed version of Tamanu and tries to
	/// look for an appropriate config. If both central and facility servers are present and
	/// configured, it will pick one arbitrarily.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-k, -kind central|facility`"))]
	#[arg(short, long)]
	pub kind: Option<ApiServerKind>,

	/// Connect to postgres with a different username.
	///
	/// This may prompt for a password depending on your local settings and pg_hba config.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-u, -U, --username STRING`"))]
	#[arg(short = 'U', long, alias = "u")]
	pub username: Option<String>,

	/// Enable write mode for this psql.
	///
	/// By default we set `TRANSACTION READ ONLY` for the session, which prevents writes. To enable
	/// writes, either pass this flag, or call `SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE;`
	/// within the session.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-W, --write`"))]
	#[arg(short = 'W', long)]
	pub write: bool,
}

/// The Tamanu config only describing the part `psql` needs
#[derive(serde::Deserialize, Debug)]
struct Config {
	db: Db,
}

#[derive(serde::Deserialize, Debug)]
struct Db {
	name: String,
	username: String,
	password: String,
}

pub async fn run(ctx: Context<TamanuArgs, PsqlArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;

	let kind = match ctx.args_sub.kind {
		Some(kind) => kind,
		None => find_package(&root)?,
	};
	info!(?kind, "using");

	let config_value = merge_json(
		package_config(&root, kind.package_name(), "default.json5")?,
		package_config(&root, kind.package_name(), "local.json5")?,
	);

	let config: Config = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing of Tamanu config failed")?;
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

	let psql_path = find_postgres_bin("psql")?;
	let version = String::from_utf8(
		duct::cmd!(&psql_path, "--version")
			.stdout_capture()
			.run()
			.into_diagnostic()?
			.stdout,
	)
	.into_diagnostic()?
	.split(|c: char| c.is_whitespace() || c == '.')
	.find_map(|word| u8::from_str(word).ok())
	.unwrap_or(12); // 12 is the lowest version we can encounter

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

	// Use the default host, which is the localhost via Unix-domain socket on Unix or TCP/IP on Windows
	duct::cmd!(psql_path, "--dbname", name, "--username", username)
		.env("PSQLRC", rc.path())
		.env("PGPASSWORD", password)
		.run()
		.into_diagnostic()
		.wrap_err("failed to execute psql")?;

	Ok(())
}
