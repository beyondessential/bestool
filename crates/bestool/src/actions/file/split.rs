use std::{
	collections::BTreeMap,
	io::{stderr, IsTerminal as _},
	num::NonZero,
	path::PathBuf,
};

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use miette::{miette, Context as _, IntoDiagnostic as _, Result};
use tokio::{
	fs::{self, create_dir_all},
	io::{AsyncReadExt as _, AsyncWriteExt as _},
};
use tokio_util::io::InspectReader;
use tracing::{debug, instrument};

use super::{Context, FileArgs};

/// Split a file into fixed-size chunks.
///
/// We sometimes deal with very large files. Uploading them in one go over an unreliable connection
/// can be a painful experience, and in some cases not succeed. This option provides a lo-fi
/// solution to the problem, by splitting a file into smaller chunks. It is then a lot easier to
/// upload the chunks and retry on error or after network failures by re-uploading chunks missing on
/// the remote; `rclone sync` can do this for example.
///
/// The file chunks are written into a directory named after the original file, including the
/// extension. This makes the remote's job simpler: take all the chunks and re-assemble into one
/// file, naming it the same as the containing directory.
///
/// A metadata file is also written. This is a JSON file which contains the number of chunks
/// created, a checksum over the whole file, and a checksum for each chunk. This can be used by the
/// re-assembler to check whether all chunks are available, and verify integrity. The `join` sibling
/// subcommand provides such a re-assembler, or you can simply use `cat` (without integrity checks).
///
/// The checksums are compatible with the ones written and verified by the `crypto hash` subcommand.
#[derive(Debug, Clone, Parser)]
pub struct SplitArgs {
	/// Path to the file to be split.
	pub input: PathBuf,

	/// Path to the output directory.
	///
	/// Cannot be the same directory as contains the input file.
	pub output: PathBuf,

	/// The chunk size in mibibytes.
	///
	/// Takes a non-zero integer size in mibibytes.
	///
	/// If not present, the default is to pick a chunk size between 8 MiB and 64 MiB inclusive, such
	/// that the input file is divided in 1000 chunks. The resulting size is rounded to the nearest
	/// 8 KiB, to make copying and disk usage more efficient.
	#[arg(long, short)]
	pub size: Option<NonZero<u16>>,
}

pub async fn run(ctx: Context<FileArgs, SplitArgs>) -> Result<()> {
	let SplitArgs {
		input,
		output,
		size,
	} = ctx.args_sub;

	let chunk_size = size.map(ChunkSize::Mib).unwrap_or_default();
	copy_into_chunks(&input, output, chunk_size).await
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum ChunkSize {
	#[default]
	Auto,
	Mib(NonZero<u16>),
}

const MIBIBYTE: u64 = 1024 * 1024;
const MAX_AUTO_CHUNKS: u64 = 1000;
const MINPAGE: u64 = 8192;
// We round chunk sizes so they always fall on the disk page size for best write and storage perf

impl ChunkSize {
	#[instrument(level = "debug")]
	fn max_chunk_bytes(self, full_size: u64) -> u64 {
		match self {
			Self::Mib(mib) => {
				let chunk_bytes = u64::from(mib.get()) * MIBIBYTE;
				if full_size < chunk_bytes {
					debug!(full_size, chunk_bytes, "full size is less than chunk");
					full_size
				} else {
					chunk_bytes
				}
			}
			Self::Auto => {
				// SAFETY: constants
				let if_8_mib =
					Self::Mib(unsafe { NonZero::new_unchecked(8) }).max_chunk_bytes(full_size);
				let if_64_mib =
					Self::Mib(unsafe { NonZero::new_unchecked(64) }).max_chunk_bytes(full_size);
				let if_max_chunks = (full_size / MAX_AUTO_CHUNKS / MINPAGE) * MINPAGE;

				debug!(if_8_mib, if_64_mib, if_max_chunks, "auto chunk size parameters");
				if_max_chunks.min(if_64_mib).max(if_8_mib)
			}
		}
	}
}

#[derive(Debug, serde::Serialize)]
pub(super) struct ChunkedMetadata {
	pub full_size: u64,
	pub full_sum: String,
	pub chunk_n: u64,
	pub chunk_size: u64,
	pub chunks: BTreeMap<String, String>,
}

#[instrument(level = "debug")]
pub(crate) async fn copy_into_chunks(
	input: &PathBuf,
	target_dir: PathBuf,
	chunk_size: ChunkSize,
) -> Result<()> {
	let target_dir = target_dir.join(
		input
			.file_name()
			.ok_or_else(|| miette!("input is not a file"))?,
	);

	let mut input = fs::File::open(input)
		.await
		.into_diagnostic()
		.wrap_err("opening input file")?;

	let input_length = input
		.metadata()
		.await
		.into_diagnostic()
		.wrap_err("reading input file size")?
		.len();

	let chunk_size = chunk_size.max_chunk_bytes(input_length);
	let n_chunks = input_length.div_ceil(chunk_size);
	let chunk_digits = usize::try_from(n_chunks.ilog10() + 1).unwrap();

	debug!(chunk_size, n_chunks, chunk_digits, input_length, ?target_dir, "chunking parameters");

	let mut chunks = BTreeMap::new();

	let pb = if stderr().is_terminal() {
		let style = ProgressStyle::default_bar()
			.template("[{bar:.green/blue}] {wide_msg} {binary_bytes}/{binary_total_bytes} ({eta})")
			.expect("BUG: progress bar template invalid");
		ProgressBar::new(input_length).with_style(style)
	} else {
		ProgressBar::hidden()
	};

	create_dir_all(&target_dir)
		.await
		.into_diagnostic()
		.wrap_err("creating output directory")?;

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
		full_size: input_length,
		full_sum: format!("b3:{}", whole_hash.finalize().to_hex()),
		chunk_n: n_chunks,
		chunk_size,
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
