use std::{
	io,
	path::{Path, PathBuf},
};

use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::io::AsyncWriteExt as _;
use tokio_tar::{Builder, HeaderMode};
use tracing::warn;

use crate::actions::{
	caddy::configure_tamanu::DEFAULT_CADDYFILE_PATH,
	crypto::{copy_encrypting, read_age_key},
	tamanu::{
		backup::{make_backup_filename, TamanuConfig},
		config::{find_config_dir, load_config},
		find_package, find_tamanu, TamanuArgs,
	},
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
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--write-to PATH`"))]
	write_to: String,

	/// Path to the Caddyfile.
	#[arg(long, default_value = DEFAULT_CADDYFILE_PATH)]
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--caddyfile-path PATH`"))]
	caddyfile_path: PathBuf,

	/// Exclude extra metadata such as ownership and mod/access times.
	#[arg(long, default_value_t = false)]
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--deterministic`, default false"))]
	deterministic: bool,

	/// Output the backup encrypted in the same way the "crypto encrypt" command does.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--encrypt-with-pubkey PATH`"))]
	#[arg(long)]
	pub encrypt_with_pubkey: Option<PathBuf>,
}

pub async fn run(ctx: Context<TamanuArgs, BackupConfigsArgs>) -> Result<()> {
	let caddyfile_path = ctx.args_sub.caddyfile_path;

	let (_, root) = find_tamanu(&ctx.args_top)?;
	let kind = find_package(&root);
	let config_value = load_config(&root, kind.package_name())?;

	let config: TamanuConfig = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing of Tamanu config failed")?;

	let pm2_config_path = root.join("pm2.config.cjs");

	let output_path = Path::new(&ctx.args_sub.write_to).join(make_backup_filename(&config, "tar"));
	let output_path = if ctx.args_sub.encrypt_with_pubkey.is_some() {
		let mut encrypted_path = output_path.clone().into_os_string();
		encrypted_path.push(".age");
		encrypted_path.into()
	} else {
		output_path
	};

	let mut archive_builder = Builder::new(Vec::new());
	if ctx.args_sub.deterministic {
		archive_builder.mode(HeaderMode::Deterministic);
	}
	fn ignore_not_found(err: io::Error) -> io::Result<()> {
		if err.kind() == io::ErrorKind::NotFound {
			warn!("Skipping a file while archiving: {err}");
			Ok(())
		} else {
			Err(err)
		}
	}
	archive_builder
		.append_path_with_name(caddyfile_path, "Caddyfile")
		.await
		.or_else(ignore_not_found)
		.into_diagnostic()
		.wrap_err("writing the backup")?;
	archive_builder
		.append_path_with_name(pm2_config_path, "pm2.config.cjs")
		.await
		.or_else(ignore_not_found)
		.into_diagnostic()
		.wrap_err("writing the backup")?;
	if let Some(path) = find_config_dir(&root, kind.package_name(), "local.json5") {
		archive_builder
			.append_path_with_name(path, "local.json5")
			.await
			.into_diagnostic()
			.wrap_err("writing the backup")?;
	} else {
		warn!("Skipping local.json5 while archiving: the file is not found");
	}
	if let Some(path) = find_config_dir(&root, kind.package_name(), "production.json5") {
		archive_builder
			.append_path_with_name(path, "production.json5")
			.await
			.into_diagnostic()
			.wrap_err("writing the backup")?;
	} else {
		warn!("Skipping production.json5 while archiving: the file is not found");
	}

	let archive = archive_builder
		.into_inner()
		.await
		.into_diagnostic()
		.wrap_err("finalising the backup")?;

	let mut file = tokio::fs::File::create_new(output_path)
		.await
		.into_diagnostic()
		.wrap_err("creating the destination")?;

	if let Some(pubkey_path) = ctx.args_sub.encrypt_with_pubkey {
		let public_key = read_age_key(&pubkey_path).await?;
		copy_encrypting(&mut archive.as_slice(), &mut file, &public_key).await?;
	} else {
		tokio::io::copy(&mut archive.as_slice(), &mut file)
			.await
			.into_diagnostic()
			.wrap_err("writing the backup")?;

		file.shutdown()
			.await
			.into_diagnostic()
			.wrap_err("closing the encrypted output")?;
	}
	Ok(())
}
