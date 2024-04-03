use std::ffi::OsString;
use std::path::Path;
use std::{fs, path::PathBuf};

use clap::Parser;
use miette::{miette, Context as _, IntoDiagnostic, Result};
use tracing::{debug, info, instrument};

use crate::actions::Context;

use super::config::{merge_json, package_config};
use super::{find_tamanu, TamanuArgs};

/// Connect to Tamanu's db via `psql`.
#[derive(Debug, Clone, Parser)]
pub struct PsqlArgs {
	/// Package to look at. By default, the command finds installed version automatically
	/// If both central and facility server were present, there's no guarantee which server this picks
	#[arg(short, long)]
	pub package: Option<String>,

	/// Include defaults
	#[arg(short = 'D', long)]
	pub defaults: bool,

	/// If given, this overwrites the username in the config and prompts for a password
	#[arg(short, long)]
	pub username: Option<String>,
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

	let package = match ctx.args_sub.package {
		Some(package) => package,
		None => find_package(&root)?,
	};
	info!(?package, "using");

	let config_value = if ctx.args_sub.defaults {
		merge_json(
			package_config(&root, &package, "default.json5")?,
			package_config(&root, &package, "local.json5")?,
		)
	} else {
		package_config(&root, &package, "local.json5")?
	};

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

	let psql_path = find_psql().wrap_err("failed to find psql executable")?;
	// Use the default host, which is the localhost via Unix-domain socket on Unix or TCP/IP on Windows
	duct::cmd!(psql_path, "--dbname", name, "--username", username,)
		.env("PGPASSWORD", password)
		.env("PSQL_HISTORY", root.with_file_name("psql.history"))
		.run()
		.into_diagnostic()
		.wrap_err("failed to execute psql")?;

	Ok(())
}

fn find_package(root: impl AsRef<Path>) -> Result<String> {
	fs::read_dir(root.as_ref().join("packages"))
		.into_diagnostic()?
		.filter_map(|res| res.map(|d| d.file_name().into_string().ok()).transpose())
		.find(|res| {
			res.as_ref()
				.map(|dir_name| dir_name == "central-server" || dir_name == "facility-server")
				.unwrap_or(true)
		})
		.ok_or_else(|| miette!("Tamanu servers not found"))?
		.into_diagnostic()
}

#[instrument(level = "debug")]
fn find_psql() -> Result<OsString> {
	// On Windows, find `psql` assuming the standard instllation using the instller
	// because PATH on Windows is not reliable.
	// See https://github.com/rust-lang/rust/issues/37519
	if cfg!(windows) {
		let root = r"C:\Program Files\PostgreSQL";
		let version = fs::read_dir(root)
			.into_diagnostic()?
			.inspect(|res| debug!(?res, "reading PostgreSQL installation"))
			.filter_map(|res| {
				res.map(|dir| {
					dir.file_name()
						.into_string()
						.ok()
						.filter(|name| name.parse::<u32>().is_ok())
				})
				.transpose()
			})
			// Use `u32::MAX` in case of `Err` so that we always catch IO errors.
			.max_by_key(|res| {
				res.as_ref()
					.cloned()
					.map(|n| n.parse::<u32>().unwrap())
					.unwrap_or(u32::MAX)
			})
			.ok_or_else(|| miette!("the Postgres root {root} is empty"))?
			.into_diagnostic()?;

		Ok([root, version.as_str(), r"bin\psql.exe"]
			.iter()
			.collect::<PathBuf>()
			.into_os_string())
	} else {
		Ok("psql".into())
	}
}