use std::{
	fs::{create_dir_all, File},
	io::{self, copy},
	path::{Path, PathBuf},
};

use algae_cli::keys::KeyArgs;
use chrono::Utc;
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use reqwest::Url;
use tracing::{debug, error, warn};
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, CompressionMethod, DateTime, ZipWriter};

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

/// Backup local Tamanu-related config files to a zip archive.
///
/// The output will be written to a file "{current_datetime}-{host_name}.config.zip".
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
	/// Only files with the `.config.zip` or the `.config.zip.age` extensions
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

	#[command(flatten)]
	pub key: KeyArgs,
}

fn zip_options() -> SimpleFileOptions {
	SimpleFileOptions::default()
		.unix_permissions(0o644)
		.compression_method(CompressionMethod::Zstd)
		.compression_level(Some(16))
}

fn add_file_impl(
	zip: &mut ZipWriter<&mut File>,
	path: &Path,
	name: &Path,
) -> Result<()> {
	debug!("trying to store file {path:?} at {name:?}");
	let mut file = File::open(path)
		.inspect_err(|err| {
			if err.kind() == io::ErrorKind::NotFound {
				debug!("skipping {path:?} because it doesn't exist");
			} else {
				warn!("skipping {path:?} because {err}");
			}
		})
		.into_diagnostic()?;

	zip.start_file_from_path(name, zip_options())
		.into_diagnostic()?;

	let bytes = copy(&mut file, zip).into_diagnostic()?;
	debug!(?bytes, "zipped file {path:?} at {name:?}");

	Ok(())
}

fn add_file(
	zip: &mut ZipWriter<&mut File>,
	path: impl AsRef<Path>,
	name: impl AsRef<Path>,
) -> bool {
	let path = path.as_ref();
	let name = name.as_ref();
	add_file_impl(zip, path, name).map_or(false, |_| true)
}

fn add_dir(
	zip: &mut ZipWriter<&mut File>,
	path: impl AsRef<Path>,
	at: impl AsRef<Path>,
) -> Result<bool> {
	let path = path.as_ref();
	let at = at.as_ref();
	debug!("trying to store dir {path:?} at {at:?}");
	if !path.exists() {
		debug!("skipping {path:?} because it doesn't exist");
		return Ok(false);
	}

	let mut success = false;
	for entry in WalkDir::new(path).follow_links(true) {
		let entry = match entry {
			Ok(e) => e,
			Err(err) => {
				warn!("skipping an entry in {path:?} because {err}");
				continue;
			}
		};

		let zip_path = if let Ok(file_path) = entry.path().strip_prefix(path) {
			at.join(file_path)
		} else {
			error!(
				"file at {:?} is not within search, this should be impossible, skipping",
				entry.path()
			);
			continue;
		};

		if entry.file_type().is_dir() {
			debug!("creating {zip_path:?} dir entry");
			zip.add_directory_from_path(zip_path, zip_options())
				.into_diagnostic()
				.wrap_err("writing directory entry failed, which is fatal")?;
			continue;
		} else if !entry.file_type().is_file() {
			debug!("skipping {zip_path:?} because it's not a file");
			continue;
		}

		success = add_file(zip, entry.path(), &zip_path);
	}

	Ok(success)
}

fn make_backup_filename(config: &TamanuConfig) -> PathBuf {
	let output_date = now_time(&Utc).format("%Y-%m-%d_%H%M");
	let canonical_host_name = Url::parse(&config.canonical_host_name).ok();
	let output_name = canonical_host_name
		.as_ref()
		.and_then(|url| url.host_str())
		.unwrap_or(&config.canonical_host_name);

	format!("{output_date}-{output_name}.config.zip").into()
}

pub async fn run(ctx: Context<TamanuArgs, BackupConfigsArgs>) -> Result<()> {
	create_dir_all(&ctx.args_sub.write_to)
		.into_diagnostic()
		.wrap_err("creating dest dir")?;

	let (_, root) = find_tamanu(&ctx.args_top)?;
	let kind = find_package(&root);
	let config_value = load_config(&root, kind.package_name())?;

	let config: TamanuConfig = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing tamanu config")?;

	let output = Path::new(&ctx.args_sub.write_to).join(make_backup_filename(&config));

	let mut file = std::fs::File::create_new(&output)
		.into_diagnostic()
		.wrap_err_with(|| format!("opening file {output:?}"))?;

	let mut zip = ZipWriter::new(&mut file);

	let mut got_caddy = add_dir(&mut zip, "/etc/caddy", "caddy")?;
	if !got_caddy {
		got_caddy = add_file(
			&mut zip,
			r"C:\Caddy\Caddyfile",
			"caddy/Caddyfile",
		);
	}
	if !got_caddy {
		got_caddy = add_file(
			&mut zip,
			r"C:\Caddy\Caddyfile.txt",
			"caddy/Caddyfile",
		);
	}
	if !got_caddy {
		error!("could not find a caddy to backup");
	}

	add_dir(&mut zip, "/etc/tamanu", "etc-tamanu")?;

	add_file(
		&mut zip,
		root.join("pm2.config.cjs"),
		"pm2.config.cjs",
	);
	add_dir(
		&mut zip,
		root.join("alerts"),
		"alerts/version",
	)?;
	add_dir(
		&mut zip,
		r"C:\Tamanu\alerts",
		"alerts/global",
	)?;
	if let Some(path) = find_config_dir(&root, kind.package_name(), ".") {
		add_dir(&mut zip, path, kind.package_name())?;
	}

	zip.finish()
		.into_diagnostic()
		.wrap_err("finalising archive")?;

	file.sync_all()
		.into_diagnostic()
		.wrap_err("fsyncing zip file")?;

	process_backup(
		output,
		&ctx.args_sub.write_to,
		ctx.args_sub.then_copy_to.as_deref(),
		ctx.args_sub.keep_days,
		".config.zip",
		ctx.args_sub.key,
	)
	.await?;

	Ok(())
}
