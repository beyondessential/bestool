use std::path::{Path, PathBuf};

use chrono::Local;
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::io::AsyncWriteExt as _;
use tokio_tar::Builder;
use tracing::{debug, info};

use crate::actions::{
	caddy::configure_tamanu::DEFAULT_CADDYFILE_PATH,
	tamanu::{find_package, find_tamanu, TamanuArgs},
	Context,
};

/// Backup a local Tamanu-related config files to a tar archive.
///
/// The output will be written to a file "{current_datetime}-{host_name}-{database_name}.tar".
#[derive(Debug, Clone, Parser)]
pub struct BackupConfigsArgs {
	/// The destination directory the output will be written to.
	#[cfg_attr(windows, arg(long, default_value = r"C:\Backup"))]
	#[cfg_attr(not(windows), arg(long, default_value = "/opt/tamanu-backup"))]
	write_to: String,

	/// Path to the Caddyfile.
	#[arg(long, default_value = DEFAULT_CADDYFILE_PATH)]
	pub caddyfile_path: PathBuf,
}

pub async fn run(ctx: Context<TamanuArgs, BackupConfigsArgs>) -> Result<()> {
	let caddyfile_path = ctx.args_sub.caddyfile_path;

	let (_, root) = find_tamanu(&ctx.args_top)?;
	let kind = find_package(&root)?;
	info!(?root, ?kind, "using this Tamanu for config");
	let tamanu_config_path = root
		.join("packages")
		.join(kind.package_name())
		.join("config");

	let pm2_config_path = root.join("pm2.config.cjs");

	let output_date = Local::now().format("%Y-%m-%d_%H%M");
	// let canonical_host_name = Url::parse(&config.canonical_host_name).ok();
	let output_path = Path::new(&ctx.args_sub.write_to).join(format!(
		"{output_date}-configs.tar",
		// Extract the host section since "canonical_host_name" is a full URL, which is not
		// suitable for a file name.
		// output_name = canonical_host_name
		// 	.as_ref()
		// 	.and_then(|url| url.host_str())
		// 	.unwrap_or(&config.canonical_host_name),
	));

	let file = tokio::fs::File::create_new(output_path)
		.await
		.into_diagnostic()
		.wrap_err("creating the destination")?;

	let mut archive_builder = Builder::new(tokio::io::BufWriter::new(file));
	archive_builder
		.append_path_with_name(caddyfile_path, "Caddyfile")
		.await
		.into_diagnostic()
		.wrap_err("writing the backup")?;
	archive_builder
		.append_path_with_name(pm2_config_path, "pm2.config.cjs")
		.await
		.into_diagnostic()
		.wrap_err("writing the backup")?;
	archive_builder
		.append_path_with_name(tamanu_config_path.join("local.json5"), "local.json5")
		.await
		.into_diagnostic()
		.wrap_err("writing the backup")?;
	archive_builder
		.append_path_with_name(
			tamanu_config_path.join("production.json5"),
			"production.json5",
		)
		.await
		.into_diagnostic()
		.wrap_err("writing the backup")?;

	archive_builder
		.into_inner()
		.await
		.into_diagnostic()
		.wrap_err("writing the backup")?
		.flush()
		.await
		.into_diagnostic()?;
	Ok(())
}
