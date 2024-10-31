use std::{fs, path::Path};

use chrono::Local;
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use tracing::{debug, info};

use crate::actions::{
	tamanu::{
		config::{merge_json, package_config},
		find_package, find_postgres_bin, find_tamanu, TamanuArgs,
	},
	Context,
};

/// Dump a local Tamanu DB using "pg_dump".
///
/// This finds the database from the Tamanu's configuration. The output will be written to a file
/// "{current_datetime}-{host_name}-{database_name}.dump".
///
/// By default, this excludes tables "sync_snapshots.*" and "fhir.jobs".
#[derive(Debug, Clone, Parser)]
pub struct BackupArgs {
	/// The compression level to use.
	///
	/// This is simply passed to the "--compress" option of "pg_dump".
	#[arg(long, default_value_t = 3)]
	compression_level: u32,

	/// The destination directory the output will be written to.
	#[cfg_attr(windows, arg(long, default_value = r"C:\Backup"))]
	#[cfg_attr(not(windows), arg(long, default_value = "/backup"))]
	write_to: String,

	/// TODO:
	#[arg(long)]
	then_copy_to: Option<String>,

	/// Enable the lean backup
	///
	/// The lean backup excludes more tables: "logs.*", "reporting.*" and "public.attachments".
	#[arg(long, default_value_t = false)]
	lean: bool,
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct TamanuConfig {
	canonical_host_name: String,
	db: TamanuDb,
}

#[derive(serde::Deserialize, Debug)]
struct TamanuDb {
	name: String,
	username: String,
	password: String,
}

pub async fn run(ctx: Context<TamanuArgs, BackupArgs>) -> Result<()> {
	// TODO: # Use two processor cores at most
	// $thisProcess = [System.Diagnostics.Process]::GetCurrentProcess();
	// $thisProcess.ProcessorAffinity = 3;

	let (_, root) = find_tamanu(&ctx.args_top)?;

	let kind = find_package(&root)?;
	info!(?root, ?kind, "using this Tamanu for config");

	let config_value = merge_json(
		package_config(&root, kind.package_name(), "default.json5")?,
		package_config(&root, kind.package_name(), "local.json5")?,
	);

	let config: TamanuConfig = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing of Tamanu config failed")?;
	debug!(?config, "parsed Tamanu config");

	let output_date = Local::now().format("%Y-%m-%d_%H%M");
	let output_name = config
		.canonical_host_name
		.strip_prefix("http://")
		.or_else(|| config.canonical_host_name.strip_prefix("https://"))
		.unwrap_or(&config.canonical_host_name);
	let output = Path::new(&ctx.args_sub.write_to).join(format!(
		"{output_date}-{output_name}-{db}.dump",
		db = config.db.name
	));

	let pg_dump = find_postgres_bin("pg_dump")?;

	// Use the default host, which is the localhost via Unix-domain socket on Unix or TCP/IP on Windows
	#[rustfmt::skip]
	if ctx.args_sub.lean {
		info!(?output, "writing lean backup");

		duct::cmd!(
			pg_dump,
            "--username", config.db.username,
			"--verbose",
			"--exclude-table", "sync_snapshots.*",
			"--exclude-table-data", "fhir.*",
			"--exclude-table-data", "logs.*",
			"--exclude-table-data", "reporting.*",
			"--exclude-table-data", "public.attachments",
			"--format", "c",
			"--compress", ctx.args_sub.compression_level.to_string(),
			"--file", &output,
			config.db.name
		)
	} else {
		info!(?output, "writing full backup to");

		duct::cmd!(
			pg_dump,
            "--username", config.db.username,
			"--verbose",
			"--exclude-table", "sync_snapshots.*",
			"--exclude-table-data", "fhir.jobs",
			"--format", "c",
			"--compress", ctx.args_sub.compression_level.to_string(),
			"--file", &output,
			config.db.name
		)
	}
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
