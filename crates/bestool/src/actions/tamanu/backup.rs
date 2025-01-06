use std::{
	ffi::{OsStr, OsString},
	fs,
	path::{Path, PathBuf},
	time::{Duration, SystemTime},
};

use algae_cli::{files::encrypt_file, keys::KeyArgs};
use chrono::Utc;
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use reqwest::Url;
use tokio::fs::remove_file;
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
///
/// If `--key` or `--key-file` is provided, the backup file will be encrypted. Note that this is
/// done by first writing the plaintext backup file to disk, then encrypting, and finally deleting
/// the original. That effectively requires double the available disk space, and the plaintext file
/// is briefly available on disk. This limitation may be lifted in the future.
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
	pub write_to: PathBuf,

	/// The file path to copy the written backup.
	///
	/// The backup will stay as is in "write_to".
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--then-copy-to PATH`"))]
	#[arg(long)]
	pub then_copy_to: Option<PathBuf>,

	/// Take a lean backup instead.
	///
	/// The lean backup excludes more tables: "logs.*", "reporting.*" and "public.attachments".
	///
	/// These thus are not suitable for recovery, but can be used for analysis.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--lean`"))]
	#[arg(long, default_value_t = false)]
	pub lean: bool,

	/// Delete backups and copies that are older than N days.
	///
	/// Only files with the `.dump` or the `.dump.age` extensions are deleted.
	/// Subfolders are not recursed into.
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

	/// Additional, arbitrary arguments to pass to "pg_dump"
	///
	/// If it has dashes (like "--password pass"), you need to prefix this with two dashes:
	///
	/// ```plain
	/// bestool tamanu backup -- --password pass
	/// ```
	#[arg(trailing_var_arg = true, verbatim_doc_comment)]
	pub args: Vec<OsString>,

	#[command(flatten)]
	pub key: KeyArgs,
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
	let config_value = load_config(&root, kind.package_name())?;

	let pg_dump = find_postgres_bin("pg_dump")?;

	let config: TamanuConfig = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing of Tamanu config failed")?;
	debug!(?config, "parsed Tamanu config");

	let key = ctx.args_sub.key.get_public_key().await?;

	let output = ctx
		.args_sub
		.write_to
		.join(make_backup_filename(&config, "dump"));

	// Use the default host, which is the localhost via Unix-domain socket on Unix or TCP/IP on Windows
	#[rustfmt::skip]
	let (backup_type, excluded_tables) = if ctx.args_sub.lean {
		(
			"lean",
			vec![
				"--exclude-table", "sync_snapshots.*",
				"--exclude-table-data", "fhir.*",
				"--exclude-table-data", "logs.*",
				"--exclude-table-data", "reporting.*",
				"--exclude-table-data", "public.attachments",
			]
			.into_iter()
			.map(Into::<OsString>::into),
		)
	} else {
		(
			"full",
			vec![
				"--exclude-table", "sync_snapshots.*",
				"--exclude-table-data", "fhir.jobs",
			]
			.into_iter()
			.map(Into::<OsString>::into),
		)
	};
	info!(?output, "writing {backup_type} backup");

	#[rustfmt::skip]
	duct::cmd(
		pg_dump,
		[
			"--username".into(), config.db.username.into(),
			"--verbose".into(),
			"--format".into(), "c".into(),
			"--compress".into(), ctx.args_sub.compression_level.to_string().into(),
			"--file".into(), output.clone().into(),
			"--dbname".into(), config.db.name.into(),
		]
		.into_iter()
		.chain(excluded_tables)
		.chain(ctx.args_sub.args),
	)
	.env("PGPASSWORD", config.db.password)
	.run()
	.into_diagnostic()
	.wrap_err("executing pg_dump")?;

	let output = if let Some(key) = key {
		let mut encrypted_path = output.clone().into_os_string();
		encrypted_path.push(".age");
		info!(path=?encrypted_path, "encrypting backup");
		encrypt_file(&output, &encrypted_path, key).await?;

		info!(path=?output, "deleting original");
		remove_file(output).await.into_diagnostic()?;

		encrypted_path.into()
	} else {
		output
	};

	if let Some(then_copy_to) = &ctx.args_sub.then_copy_to {
		info!(path=?then_copy_to, "copying backup");
		fs::copy(&output, then_copy_to).into_diagnostic()?;
	}

	if let Some(days) = ctx.args_sub.keep_days {
		let just_written_filename = output
			.file_name()
			.expect("from above we know it's got a filename");

		purge_old_backups(days, &ctx.args_sub.write_to, just_written_filename)
			.await
			.wrap_err("purging old backups in main target")?;

		if let Some(copies) = &ctx.args_sub.then_copy_to {
			purge_old_backups(days, copies, just_written_filename)
				.await
				.wrap_err("purging old backups in secondary target")?;
		}
	}

	Ok(())
}

#[instrument(level = "debug")]
pub fn make_backup_filename(config: &TamanuConfig, ext: &str) -> String {
	let output_date = now_time(&Utc).format("%Y-%m-%d_%H%M");

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

#[instrument(level = "debug")]
async fn purge_old_backups(
	older_than_days: u16,
	from_dir: &Path,
	exclude_filename: &OsStr,
) -> Result<()> {
	const SECONDS_IN_A_DAY: u64 = 60 * 60 * 24;
	let limit_date =
		SystemTime::now() - Duration::from_secs((older_than_days as u64) * SECONDS_IN_A_DAY);

	let mut dir = tokio::fs::read_dir(from_dir)
		.await
		.into_diagnostic()
		.wrap_err(format!("reading directory {from_dir:?}"))?;

	while let Some(entry) = dir
		.next_entry()
		.await
		.into_diagnostic()
		.wrap_err(format!("reading directory entry in {from_dir:?}"))?
	{
		let path = entry.path();

		let name = entry.file_name();
		if name == exclude_filename {
			debug!(?path, "ignoring file we just created");
			continue;
		}

		let name = name.to_string_lossy();
		if !(name.ends_with(".dump") || name.ends_with(".dump.age")) {
			debug!(?path, "ignoring file with wrong extension");
			continue;
		}

		let meta = entry
			.metadata()
			.await
			.into_diagnostic()
			.wrap_err(format!("looking up metadata for {path:?}"))?;
		let Ok(date_created) = meta.created().or_else(|_| meta.modified()) else {
			debug!(?path, "ignoring file without created/modified timestamp");
			continue;
		};

		if date_created > limit_date {
			debug!(?path, "ignoring too-new file");
			continue;
		}

		info!(?path, "deleting old backup");
		tokio::fs::remove_file(&path)
			.await
			.into_diagnostic()
			.wrap_err(format!("deleting {path:?}"))?;
	}

	Ok(())
}
