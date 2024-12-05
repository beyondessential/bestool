use std::{ffi::OsString, fs, path::Path};

use chrono::Local;
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use reqwest::Url;
use tracing::{debug, info, instrument};

use crate::{
	actions::{
		tamanu::{config::load_config, find_package, find_postgres_bin, find_tamanu, TamanuArgs},
		Context,
	},
	now_time,
};

/// Backup a local Tamanu database to a single file.
///
/// This finds the database from the Tamanu's configuration. The output will be written to a file
/// "{current_datetime}-{host_name}-{database_name}.dump".
///
/// By default, this excludes tables "sync_snapshots.*" and "fhir.jobs".
#[cfg_attr(docsrs, doc("\n\n**Command**: `bestool tamanu backup`"))]
#[derive(Debug, Clone, Parser)]
pub struct BackupArgs {
	/// The compression level to use.
	///
	/// This is simply passed to the "--compress" option of "pg_dump".
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--compression-level LEVEL`, default 3"))]
	#[arg(long, default_value_t = 3)]
	pub compression_level: u32,

	/// The destination directory the output will be written to.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--write-to PATH`"))]
	#[cfg_attr(windows, arg(long, default_value = r"C:\Backup"))]
	#[cfg_attr(not(windows), arg(long, default_value = "/opt/tamanu-backup"))]
	pub write_to: String,

	/// The file path to copy the written backup.
	///
	/// The backup will stay as is in "write_to".
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--then-copy-to PATH`"))]
	#[arg(long)]
	pub then_copy_to: Option<String>,

	/// Take a lean backup instead.
	///
	/// The lean backup excludes more tables: "logs.*", "reporting.*" and "public.attachments".
	///
	/// These thus are not suitable for recovery, but can be used for analysis.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--lean`"))]
	#[arg(long, default_value_t = false)]
	pub lean: bool,

	/// Additional, arbitrary arguments to pass to "pg_dump"
	///
	/// If it has dashes (like "--password pass"), you need to prefix this with two dashes:
	///
	/// ```plain
	/// bestool tamanu backup -- --password pass
	/// ```
	#[arg(trailing_var_arg = true)]
	pub args: Vec<OsString>,
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TamanuConfig {
	canonical_host_name: String,
	db: TamanuDb,
}

#[derive(serde::Deserialize, Debug)]
pub struct TamanuDb {
	name: String,
	username: String,
	password: String,
}

pub async fn run(ctx: Context<TamanuArgs, BackupArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;
	let kind = find_package(&root);
	let config_value = load_config(&root, &kind.package_name())?;

	let config: TamanuConfig = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing of Tamanu config failed")?;
	debug!(?config, "parsed Tamanu config");

	let output = Path::new(&ctx.args_sub.write_to).join(make_backup_filename(&config, "dump"));

	let pg_dump = find_postgres_bin("pg_dump")?;

	// Use the default host, which is the localhost via Unix-domain socket on Unix or TCP/IP on Windows
	#[rustfmt::skip]
	let excluded_tables = if ctx.args_sub.lean {
		info!(?output, "writing lean backup");

		vec!["--exclude-table", "sync_snapshots.*",
			"--exclude-table-data", "fhir.*",
			"--exclude-table-data", "logs.*",
			"--exclude-table-data", "reporting.*",
			"--exclude-table-data", "public.attachments"].into_iter().map(Into::<OsString>::into)
	} else {
		info!(?output, "writing full backup to");

		vec!["--exclude-table", "sync_snapshots.*",
			"--exclude-table-data", "fhir.jobs"].into_iter().map(Into::<OsString>::into)
	};

	duct::cmd(
		pg_dump,
		[
			Into::<OsString>::into("--username"),
			config.db.username.into(),
			"--verbose".into(),
			"--format".into(),
			"c".into(),
			"--compress".into(),
			ctx.args_sub.compression_level.to_string().into(),
			"--file".into(),
			output.clone().into(),
			"--dbname".into(),
			config.db.name.into(),
		]
		.into_iter()
		.chain(excluded_tables)
		.chain(ctx.args_sub.args),
	)
	.env("PGPASSWORD", config.db.password)
	.run()
	.into_diagnostic()
	.wrap_err("executing pg_dump")?;

	if let Some(then_copy_to) = ctx.args_sub.then_copy_to {
		info!(?then_copy_to, "copying backup");
		fs::copy(output, then_copy_to).into_diagnostic()?;
	}

	Ok(())
}

#[instrument(level = "debug")]
pub fn make_backup_filename(config: &TamanuConfig, ext: &str) -> String {
	let output_date = now_time(&Local).format("%Y-%m-%d_%H%M");

	let canonical_host_name = Url::parse(&config.canonical_host_name).ok();
	format!(
		"{output_date}-{output_name}-{db}.{ext}",
		// Extract the host section since "canonical_host_name" is a full URL, which is not
		// suitable for a file name.
		output_name = canonical_host_name
			.as_ref()
			.and_then(|url| url.host_str())
			.unwrap_or(&config.canonical_host_name),
		db = config.db.name,
	)
}
