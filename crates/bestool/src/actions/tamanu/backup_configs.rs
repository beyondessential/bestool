use std::{
	io,
	path::{Path, PathBuf},
};

use algae_cli::keys::KeyArgs;
use chrono::Utc;
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use reqwest::Url;
use tokio::{
	fs::{create_dir_all, File},
	io::{AsyncWriteExt as _, BufWriter},
};
use tokio_tar::{Builder, HeaderMode};
use tracing::{debug, error, info, warn};

use crate::{
	actions::{
		tamanu::{
			backup::TamanuConfig,
			config::{find_config_dir, load_config},
			find_package, find_tamanu, TamanuArgs,
		},
		Context,
	},
	now_time,
};

use super::backup::process_backup;

/// Backup local Tamanu-related config files to a tar archive.
///
/// The output will be written to a file "{current_datetime}-{host_name}.config.tar".
///
/// If `--key` or `--key-file` is provided, the backup file will be encrypted. Note that this is
/// done by first writing the plaintext backup file to disk, then encrypting, and finally deleting
/// the original. That effectively requires double the available disk space, and the plaintext file
/// is briefly available on disk. This limitation may be lifted in the future.
#[derive(Debug, Clone, Parser)]
pub struct BackupConfigsArgs {
	/// The destination directory the output will be written to.
	#[cfg_attr(windows, arg(long, default_value = r"C:\Backup\Config"))]
	#[cfg_attr(not(windows), arg(long, default_value = "/opt/tamanu-backup/config"))]
	pub write_to: PathBuf,

	/// The file path to copy the written backup.
	///
	/// The backup will stay as is in "write_to".
	#[arg(long)]
	pub then_copy_to: Option<PathBuf>,

	/// Delete backups and copies that are older than N days.
	///
	/// Only files with the `.config.tar` or the `.config.tar.age` extensions
	/// are deleted. Subfolders are not recursed into.
	///
	/// If this option is not provided, a single backup is taken and no
	/// deletions are executed.
	///
	/// Backup deletion always occurs after the backup is taken, so that if the
	/// process fails for some reason, existing (presumed valid) backups remain.
	///
	/// If `--then-copy-to` is provided, also deletes backup files there.
	#[arg(long)]
	pub keep_days: Option<u16>,

	/// Exclude extra metadata such as ownership and mod/access times.
	#[arg(long, default_value_t = false)]
	pub deterministic: bool,

	#[command(flatten)]
	pub key: KeyArgs,
}

async fn add_file(builder: &mut Builder<BufWriter<File>>, path: impl AsRef<Path>, name: &str) -> bool {
	let path = path.as_ref();
	debug!("trying to store {path:?} at {name}");
	builder
		.append_path_with_name(path, name)
		.await
		.map(|_| {
			info!("stored {path:?}");
			true
		})
		.unwrap_or_else(|err| {
			if err.kind() == io::ErrorKind::NotFound {
				debug!("skipping {path:?} because it doesn't exist");
			} else {
				warn!("skipping {path:?} because {err}");
			}
			false
		})
}

async fn add_dir(builder: &mut Builder<BufWriter<File>>, path: impl AsRef<Path>, at: &str) -> bool {
	let path = path.as_ref();
	debug!("trying to store {path:?} at {at}");
	builder
		.append_dir_all(path, at)
		.await
		.map(|_| {
			info!("stored {path:?}");
			true
		})
		.unwrap_or_else(|err| {
			if err.kind() == io::ErrorKind::NotFound {
				debug!("skipping {path:?} because it doesn't exist");
			} else {
				warn!("skipping {path:?} because {err}");
			}
			false
		})
}

fn make_backup_filename(config: &TamanuConfig) -> PathBuf {
	let output_date = now_time(&Utc).format("%Y-%m-%d_%H%M");
	let canonical_host_name = Url::parse(&config.canonical_host_name).ok();
	let output_name = canonical_host_name
		.as_ref()
		.and_then(|url| url.host_str())
		.unwrap_or(&config.canonical_host_name);

	format!("{output_date}-{output_name}.config.tar").into()
}

pub async fn run(ctx: Context<TamanuArgs, BackupConfigsArgs>) -> Result<()> {
	create_dir_all(&ctx.args_sub.write_to)
		.await
		.into_diagnostic()
		.wrap_err("creating dest dir")?;

	let (_, root) = find_tamanu(&ctx.args_top)?;
	let kind = find_package(&root);
	let config_value = load_config(&root, kind.package_name())?;

	let config: TamanuConfig = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing tamanu config")?;

	let output = Path::new(&ctx.args_sub.write_to).join(make_backup_filename(&config));

	let file = tokio::fs::File::create_new(&output)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("opening file {output:?}"))?;

	let mut builder = Builder::new(tokio::io::BufWriter::new(file));
	if ctx.args_sub.deterministic {
		builder.mode(HeaderMode::Deterministic);
	}

	let mut got_caddy = add_dir(&mut builder, "/etc/caddy", "caddy").await;
	if !got_caddy {
		got_caddy = add_file(&mut builder, r"C:\Caddy\Caddyfile", "caddy/Caddyfile").await;
	}
	if !got_caddy {
		got_caddy = add_file(&mut builder, r"C:\Caddy\Caddyfile.txt", "caddy/Caddyfile").await;
	}
	if !got_caddy {
		error!("could not find a caddy to backup");
	}

	add_dir(&mut builder, "/etc/tamanu", "etc-tamanu").await;

	add_file(&mut builder, root.join("pm2.config.cjs"), "pm2.config.cjs").await;
	add_dir(&mut builder, root.join("alerts"), "alerts/version").await;
	add_dir(&mut builder, r"C:\Tamanu\alerts", "alerts/global").await;
	if let Some(path) = find_config_dir(&root, kind.package_name(), ".") {
		add_dir(&mut builder, path, kind.package_name()).await;
	}

	builder
		.into_inner()
		.await
		.into_diagnostic()
		.wrap_err("writing tar file")?
		.flush()
		.await
		.into_diagnostic()
		.wrap_err("flushing tar file")?;

	process_backup(
		output,
		&ctx.args_sub.write_to,
		ctx.args_sub.then_copy_to.as_deref(),
		ctx.args_sub.keep_days,
		ctx.args_sub.key,
	)
	.await?;

	Ok(())
}
