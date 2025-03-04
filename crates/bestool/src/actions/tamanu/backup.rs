use std::{
	collections::BTreeMap, ffi::{OsStr, OsString}, io::{stderr, IsTerminal as _}, num::NonZero, path::{Path, PathBuf}, time::{Duration, SystemTime}
};

use algae_cli::{
	files::{encrypt_file, with_progress_bar},
	keys::KeyArgs,
};
use chrono::Utc;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::{
	fs::{self, create_dir_all},
	io::{AsyncReadExt as _, AsyncWriteExt as _},
};
use tokio_util::io::InspectReader;
use tracing::{debug, info, instrument, warn};

use crate::{
	actions::{
		tamanu::{config::load_config, find_postgres_bin, find_tamanu, TamanuArgs},
		Context,
	},
	now_time,
};

use super::config::TamanuConfig;

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
#[derive(Debug, Clone, Parser)]
pub struct BackupArgs {
	/// The compression level to use.
	///
	/// This is simply passed to the "--compress" option of "pg_dump".
	#[arg(long, default_value_t = 3)]
	pub compression_level: u32,

	/// The destination directory the output will be written to.
	#[cfg_attr(windows, arg(long, default_value = r"C:\Backup"))]
	#[cfg_attr(not(windows), arg(long, default_value = "/opt/tamanu-backup"))]
	pub write_to: PathBuf,

	/// The file path to copy the written backup.
	///
	/// The backup will stay as is in "write_to".
	#[arg(long)]
	pub then_copy_to: Option<PathBuf>,

	/// Split the copied file into fixed-sized chunks.
	///
	/// Backups can be very large files. Uploading them in one go over an unreliable connection can
	/// be a painful experience, and in some cases not succeed. This option provides a lo-fi
	/// solution to the problem, by splitting the file created by `--then-copy` into smaller chunks.
	/// It is then a lot easier to upload the chunks and retry on error or after network failures by
	/// re-uploading chunks missing on the remote; `rclone sync` can do this for example.
	///
	/// The file chunks are written into a directory named after the original file, including the
	/// extension. This makes the other side simpler: take all the chunks and re-assemble into one
	/// file, naming it the same as the containing directory.
	///
	/// A metadata file is also written. This is a JSON file which contains the number of chunks
	/// created, a checksum over the whole file, and a checksum for each chunk. This can be used by
	/// the far-side re-assembler to check whether all files are available, and verify integrity.
	///
	/// Splitting happens after encryption, if enabled.
	///
	/// Takes a non-zero integer size in mibibytes, or `0`, which will pick either 64MiB or 1000
	/// chunks (rounded to 8192 bytes), whichever makes smaller chunks, with a minimum size of 8MiB.
	#[arg(long)]
	pub then_split: Option<u16>,

	/// Take a lean backup instead.
	///
	/// The lean backup excludes more tables: "logs.*", "reporting.*" and "public.attachments".
	///
	/// These thus are not suitable for recovery, but can be used for analysis.
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

	#[arg(long, hide = true)]
	pub debug_skip_pgdump: bool,

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

pub async fn run(ctx: Context<TamanuArgs, BackupArgs>) -> Result<()> {
	create_dir_all(&ctx.args_sub.write_to)
		.await
		.into_diagnostic()
		.wrap_err("creating dest dir")?;

	let (_, root) = find_tamanu(&ctx.args_top)?;
	let config = load_config(&root, None)?;
	debug!(?config, "parsed Tamanu config");

	let pg_dump = find_postgres_bin("pg_dump")?;

	// check key
	ctx.args_sub.key.get_public_key().await?;

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

	if !ctx.args_sub.debug_skip_pgdump {
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
	} else {
		let _ = fs::File::create_new(&output).await.into_diagnostic()?;
	}

	process_backup(
		output,
		&ctx.args_sub.write_to,
		ctx.args_sub.then_copy_to.as_deref().map(|path| Then {
			copy_to: path,
			split: ctx
				.args_sub
				.then_split
				.map(|n| {
					NonZero::new(n)
						.map(ThenSplit::Mib)
						.unwrap_or(ThenSplit::Auto)
				})
				.unwrap_or(ThenSplit::No),
		}),
		ctx.args_sub.keep_days,
		".dump",
		ctx.args_sub.key,
	)
	.await?;

	Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ThenSplit {
	No,
	Auto,
	Mib(NonZero<u16>),
}

const MIBIBYTE: u64 = 1024_u64.pow(3);
const MAX_AUTO_CHUNKS: u64 = 1000;
const MINPAGE: u64 = 8192;
// We round chunk sizes so they always fall on the disk page size for best write and storage perf

impl ThenSplit {
	// None if we're not splitting
	fn max_chunk_bytes(self, full_size: u64) -> Option<u64> {
		match self {
			Self::No => None,
			Self::Mib(mib) => Some({
				let chunk_bytes = u64::from(mib.get()) * MIBIBYTE;
				if full_size < chunk_bytes {
					full_size
				} else {
					chunk_bytes
				}
			}),
			Self::Auto => {
				// SAFETY: constants
				// UNWRAP: Mib always returns Some
				let if_8_mib = Self::Mib(unsafe { NonZero::new_unchecked(8) })
					.max_chunk_bytes(full_size)
					.unwrap();
				let if_64_mib = Self::Mib(unsafe { NonZero::new_unchecked(64) })
					.max_chunk_bytes(full_size)
					.unwrap();
				let if_max_chunks = (full_size / MAX_AUTO_CHUNKS / MINPAGE) * MINPAGE;
				Some(if_max_chunks.min(if_64_mib).max(if_8_mib))
			}
		}
	}
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Then<'a> {
	pub copy_to: &'a Path,
	pub split: ThenSplit,
}

pub(crate) async fn process_backup(
	output: PathBuf,
	written_to: &Path,
	then: Option<Then<'_>>,
	keep_days: Option<u16>,
	purge_extension: &str,
	key: KeyArgs,
) -> Result<PathBuf, miette::Error> {
	let key = key.get_public_key().await?;

	let output = if let Some(key) = key {
		let mut encrypted_path = output.clone().into_os_string();
		encrypted_path.push(".age");
		info!(path=?encrypted_path, "encrypting backup");
		encrypt_file(&output, &encrypted_path, key).await?;

		info!(path=?output, "deleting original");
		fs::remove_file(output).await.into_diagnostic()?;

		encrypted_path.into()
	} else {
		output
	};

	let full_size = {
		let meta = fs::metadata(&output)
			.await
			.into_diagnostic()
			.wrap_err("opening the output (metadata)")?;
		meta.len()
	};
	info!("wrote {full_size} bytes");

	let output_filename = output
		.file_name()
		.expect("from above we know it's got a filename");

	if let Some(Then { copy_to, split }) = then {
		create_dir_all(copy_to)
			.await
			.into_diagnostic()
			.wrap_err("creating copy dest dir")?;

		let target_path = copy_to.join(output_filename);

		if let Some(chunk_size) = split.max_chunk_bytes(full_size) {
			info!(from=?output, to=?target_path, %chunk_size, "copying split backup");
			copy_into_chunks(&output, full_size, target_path, chunk_size).await?;
		} else {
			info!(from=?output, to=?target_path, "copying whole backup");

			// attempt copy first via fs, then fallback to via io
			if let Err(e) = fs::copy(&output, &target_path).await {
				warn!(?e, "fs::copy failed, falling back to io::copy");
				copy_via_io(&output, full_size, target_path).await?;
			}
		}
	}

	if let Some(days) = keep_days {
		purge_old_backups(days, written_to, output_filename, purge_extension)
			.await
			.wrap_err("purging old backups in main target")?;

		if let Some(copies) = then.map(|t| t.copy_to) {
			purge_old_backups(days, copies, output_filename, purge_extension)
				.await
				.wrap_err("purging old backups in secondary target")?;
		}
	}

	Ok(output)
}

async fn copy_via_io(
	input: &PathBuf,
	input_length: u64,
	target_path: PathBuf,
) -> Result<(), miette::Error> {
	let input = fs::File::open(input)
		.await
		.into_diagnostic()
		.wrap_err("opening the original")?;

	let mut writer = fs::File::create_new(target_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the target file")?;

	let mut reader = with_progress_bar(input_length, input);

	let bytes = tokio::io::copy(&mut reader, &mut writer)
		.await
		.into_diagnostic()
		.wrap_err("copying data in stream")?;
	debug!("copied {bytes} bytes");

	writer
		.shutdown()
		.await
		.into_diagnostic()
		.wrap_err("closing the target file")
}

#[derive(Debug, serde::Serialize)]
struct ChunkedMetadata {
	n: u64,
	sum: String,
	chunks: BTreeMap<String, String>,
}

async fn copy_into_chunks(
	input: &PathBuf,
	input_length: u64,
	target_dir: PathBuf,
	chunk_size: u64,
) -> Result<(), miette::Error> {
	let mut input = fs::File::open(input)
		.await
		.into_diagnostic()
		.wrap_err("opening the original")?;

	let n_chunks = input_length.div_ceil(chunk_size);
	let chunk_digits = usize::try_from(n_chunks.ilog10() + 1).unwrap();
	let mut chunks = BTreeMap::new();

	let pb = if stderr().is_terminal() {
		let style = ProgressStyle::default_bar()
			.template("[{bar:.green/blue}] {wide_msg} {binary_bytes}/{binary_total_bytes} ({eta})")
			.expect("BUG: progress bar template invalid");
		ProgressBar::new(input_length).with_style(style)
	} else {
		ProgressBar::hidden()
	};

	let mut whole_hash = blake3::Hasher::new();

	let mut chunk_n = 0;
	loop {
		chunk_n += 1;
		let mut chunk_hash = blake3::Hasher::new();
		let mut chunk = InspectReader::new(input.take(chunk_size), |bytes| {
			whole_hash.update(bytes);
			chunk_hash.update(bytes);
		});

		let chunk_name = format!("{chunk_n:0chunk_digits$}.chunk");
		let target_path = target_dir.join(&chunk_name);
		pb.set_message(chunk_name.clone());

		let mut writer = fs::File::create_new(&target_path)
			.await
			.into_diagnostic()
			.wrap_err("opening the target file")?;

		// TODO: checksums
		let bytes = tokio::io::copy(&mut chunk, &mut writer)
			.await
			.into_diagnostic()
			.wrap_err("copying data in stream")?;
		debug!(%chunk_n, %n_chunks, "copied {bytes} bytes");
		pb.inc(bytes);

		writer
			.shutdown()
			.await
			.into_diagnostic()
			.wrap_err("closing the target file")?;
		input = chunk.into_inner().into_inner();

		if bytes == 0 {
			let _ = fs::remove_file(target_path).await;
			break;
		}

		chunks.insert(chunk_name, format!("b3:{}", chunk_hash.finalize().to_hex()));
	}

	let meta = ChunkedMetadata {
		n: n_chunks,
		sum: format!("b3:{}", whole_hash.finalize().to_hex()),
		chunks,
	};
	let meta = serde_json::to_vec_pretty(&meta).unwrap();
	fs::write(target_dir.join("metadata.json"), meta)
		.await
		.into_diagnostic()
		.wrap_err("write metadata file")?;

	pb.finish_with_message(format!("wrote {n_chunks} chunks"));
	Ok(())
}

#[instrument(level = "debug")]
pub fn make_backup_filename(config: &TamanuConfig, ext: &str) -> String {
	let output_date = now_time(&Utc).format("%Y-%m-%d_%H%M");
	let output_name = config
		.canonical_host_name
		.as_ref()
		.and_then(|url| url.host_str())
		.unwrap_or_else(|| "localhost");

	format!(
		"{output_date}-{output_name}-{db}.{ext}",
		db = config.db.name,
	)
}

#[instrument(level = "debug")]
async fn purge_old_backups(
	older_than_days: u16,
	from_dir: &Path,
	exclude_filename: &OsStr,
	include_extension: &str,
) -> Result<()> {
	const SECONDS_IN_A_DAY: u64 = 60 * 60 * 24;
	let limit_date =
		SystemTime::now() - Duration::from_secs((older_than_days as u64) * SECONDS_IN_A_DAY);

	let mut dir = fs::read_dir(from_dir)
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
		if !(name.ends_with(include_extension)
			|| name.ends_with(&format!("{include_extension}.age")))
		{
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
		if meta.is_dir() {
			fs::remove_dir_all(&path)
				.await
				.into_diagnostic()
				.wrap_err(format!("deleting dir {path:?}"))?;
		} else {
			fs::remove_file(&path)
				.await
				.into_diagnostic()
				.wrap_err(format!("deleting file {path:?}"))?;
		}
	}

	Ok(())
}
