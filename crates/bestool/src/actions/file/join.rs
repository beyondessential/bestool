use std::{
	io::IsTerminal,
	path::{Path, PathBuf},
};

use clap::Parser;
use futures::future::join_all;
use miette::{bail, miette, IntoDiagnostic, Result, WrapErr};
use tokio::{
	fs::{self, remove_file, File},
	io::{copy_buf, empty, stdout, AsyncBufRead, AsyncReadExt, AsyncWriteExt},
};
use tracing::{info, instrument};

use super::{split::ChunkedMetadata, Context, FileArgs};

/// Join a split file.
///
/// This is the counter to `bestool file split`.
///
/// Chunked files can be joined very simply using `cat`. However, this will not verify integrity.
/// This subcommand checks that all chunks are present, that each chunk matches its checksum, and
/// that the whole file matches that checksum as well, while writing the joined file.
///
/// As a result, it is also quite a bit slower than `cat`; if you trust the input, you may want to
/// use that instead for performance.
#[derive(Debug, Clone, Parser)]
pub struct JoinArgs {
	/// Path to the directory of chunks to be joined.
	pub input: PathBuf,

	/// Path to the output directory or file.
	///
	/// If a directory is given, this cannot be the same directory as contains the input chunked
	/// directory; the name of the directory will be used as the output filename.
	///
	/// If not provided, and stdout is NOT a terminal, the output will be streamed there. Note that
	/// in that case, you should pay attention to the exit code: if it is not success, integrity
	/// checks may have failed and you should discard the obtained output.
	pub output: Option<PathBuf>,
}

pub async fn run(ctx: Context<FileArgs, JoinArgs>) -> Result<()> {
	let JoinArgs { input, output } = ctx.args_sub;

	let meta = parse_metadata(&input).await?;
	let expected_bytes = meta.full_size;
	if !verify_all_chunks_correct(&input, &meta).await {
		bail!("some chunks missing or incomplete");
	}

	let mut stream = read_from_chunks(&input, &meta).await?;
	if let Some(output) = output {
		let output = if output.is_dir() {
			output.join(
				input
					.file_name()
					.ok_or_else(|| miette!("input is not a filename"))?,
			)
		} else {
			output
		};

		let mut file = File::create_new(&output)
			.await
			.into_diagnostic()
			.wrap_err("opening output file")?;
		match copy_buf(&mut stream, &mut file)
			.await
			.into_diagnostic()
			.wrap_err("writing to file")
		{
			Err(err) => {
				// best-effort cleanup
				let _ = file.shutdown().await;
				drop(file);
				let _ = remove_file(output).await;

				Err(err)
			}
			Ok(bytes) if bytes != expected_bytes => {
				// best-effort cleanup
				let _ = file.shutdown().await;
				drop(file);
				let _ = remove_file(output).await;

				bail!("expected {expected_bytes} bytes, got {bytes} bytes");
			}
			Ok(bytes) => {
				info!("wrote {bytes} bytes");
				Ok(())
			}
		}
	} else if std::io::stdout().is_terminal() {
		Err(miette!("stdout is a terminal, not writing data there")
			.wrap_err("did you mean to write to a file? provide a second argument"))
	} else {
		let mut stdout = stdout();
		let bytes = copy_buf(&mut stream, &mut stdout)
			.await
			.into_diagnostic()
			.wrap_err("writing to file")?;

		if bytes != expected_bytes {
			bail!("expected {expected_bytes} bytes, got {bytes} bytes");
		}

		info!("wrote {bytes} bytes");
		Ok(())
	}
}

const MAX_METADATA_SIZE: u64 = 1024 * 1024; // 1 mibibyte, should be 100x more than anything reasonable

#[instrument(level = "debug")]
async fn parse_metadata(input: &Path) -> Result<ChunkedMetadata> {
	let mut file = File::open(input.join("metadata.json"))
		.await
		.into_diagnostic()
		.wrap_err("open metadata.json")?;
	let file_size = file
		.metadata()
		.await
		.into_diagnostic()
		.wrap_err("read metadata.json size")?
		.len();
	if file_size > MAX_METADATA_SIZE {
		bail!("metadata.json is way too large, this is a trap / not a valid chunked file");
	}

	// UNWRAP: MAX_METADATA_SIZE is always under a usize on supported archs
	let mut json = Vec::with_capacity(usize::try_from(file_size).unwrap());
	let bytes = file
		.read_to_end(&mut json)
		.await
		.into_diagnostic()
		.wrap_err("read metadata.json")?;
	if file_size != u64::try_from(bytes).unwrap() {
		// UNWRAP above: if we hit that unwrap we've read so much data we should have crashed
		bail!("metadata.json read was interrupted, expected {file_size} bytes and got {bytes}");
	}
	serde_json::from_slice(&json)
		.into_diagnostic()
		.wrap_err("parse metadata.json")
}

/// Check all chunks for existence and size match (no checksums)
#[instrument(level = "debug")]
async fn verify_all_chunks_correct(input: &Path, meta: &ChunkedMetadata) -> bool {
	join_all(
		meta.chunks
			.keys()
			.enumerate()
			.map(|(n, filename)| async move {
				let Ok(file_meta) = fs::metadata(input.join(filename)).await else {
					return false;
				};
				if !file_meta.is_file() {
					return false;
				}
				if file_meta.len() != meta.chunk_size && u64::try_from(n).unwrap() != meta.chunk_n {
					return false;
				}

				true
			}),
	)
	.await
	.iter()
	.all(|t| *t)
}

#[instrument(level = "debug")]
async fn read_from_chunks(input: &Path, meta: &ChunkedMetadata) -> Result<impl AsyncBufRead> {
	Ok(empty())
}
