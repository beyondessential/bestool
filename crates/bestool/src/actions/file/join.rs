use std::{
	io::{stderr, ErrorKind, IsTerminal},
	path::{Path, PathBuf},
	pin::Pin,
	task::Poll,
};

use blake3::{Hash, Hasher};
use bytes::Bytes;
use clap::Parser;
use futures::{future::join_all, stream, FutureExt, Stream, StreamExt as _, TryStreamExt as _};
use indicatif::{ProgressBar, ProgressStyle};
use miette::{bail, miette, IntoDiagnostic, Result, WrapErr};
use tokio::{
	fs::{self, remove_file, File},
	io::{copy_buf, stdout, AsyncReadExt, AsyncWriteExt},
};
use tokio_util::io::{ReaderStream, StreamReader};
use tracing::{error, info, instrument};

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

	let full_sum = Hash::from_hex(
		meta.full_sum
			.strip_prefix("b3:")
			.ok_or_else(|| miette!("full_sum has bad prefix"))?,
	)
	.into_diagnostic()
	.wrap_err("full_sum is in invalid format")?;

	let expected_bytes = meta.full_size;
	if !verify_all_chunks_correct(&input, &meta).await {
		bail!("some chunks missing or incomplete");
	}

	let pb = if stderr().is_terminal() {
		let style = ProgressStyle::default_bar()
			.template("[{bar:.green/blue}] {binary_bytes}/{binary_total_bytes} ({eta})")
			.expect("BUG: progress bar template invalid");
		ProgressBar::new(expected_bytes).with_style(style)
	} else {
		ProgressBar::hidden()
	};

	let mut hasher = Hasher::new();
	let mut stream = StreamReader::new(chunk_readers(&input, &meta).try_flatten().inspect_ok(
		|bytes| {
			hasher.update(bytes);
			pb.inc(bytes.len() as _);
		},
	));

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

				pb.abandon();
				Err(err)
			}
			Ok(bytes) if bytes != expected_bytes => {
				// best-effort cleanup
				let _ = file.shutdown().await;
				drop(file);
				let _ = remove_file(output).await;

				pb.abandon();
				bail!("expected {expected_bytes} bytes, got {bytes} bytes");
			}
			Ok(bytes) => {
				pb.finish();

				let sum = hasher.finalize();
				if sum != full_sum {
					// best-effort cleanup
					let _ = file.shutdown().await;
					drop(file);
					let _ = remove_file(output).await;

					bail!("bad checksum!\nexpected: {full_sum}\nobtained: {sum}");
				}

				info!("wrote {bytes} bytes");
				Ok(())
			}
		}
	} else if std::io::stdout().is_terminal() {
		pb.finish_and_clear();
		Err(miette!("stdout is a terminal, not writing data there")
			.wrap_err("did you mean to write to a file? provide a second argument"))
	} else {
		let mut stdout = stdout();
		let bytes = copy_buf(&mut stream, &mut stdout)
			.await
			.into_diagnostic()
			.wrap_err("writing to file")?;

		if bytes != expected_bytes {
			pb.abandon();
			bail!("expected {expected_bytes} bytes, got {bytes} bytes");
		}

		pb.finish();

		let sum = hasher.finalize();
		if sum != full_sum {
			bail!("bad checksum!\nexpected: {full_sum}\nobtained: {sum}");
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

/// Check all chunks for existence, size match, and checksum format (no checksum verification)
#[instrument(level = "debug")]
async fn verify_all_chunks_correct(input: &Path, meta: &ChunkedMetadata) -> bool {
	join_all(
		meta.chunks
			.iter()
			.enumerate()
			.map(|(n, (filename, sum))| async move {
				let Ok(file_meta) = fs::metadata(input.join(filename)).await else {
					error!(n, filename, "chunk not found");
					return false;
				};
				if !file_meta.is_file() {
					error!(n, filename, "chunk not a file");
					return false;
				}
				if file_meta.len() != meta.chunk_size
					&& u64::try_from(n + 1).unwrap() != meta.chunk_n
				{
					error!(
						n,
						filename,
						expected = meta.chunk_size,
						actual = file_meta.len(),
						"chunk not correct size"
					);
					return false;
				}
				let Some(sum) = sum.strip_prefix("b3:") else {
					error!(n, filename, sum, "chunk sum not prefixed by b3:");
					return false;
				};
				if let Err(err) = Hash::from_hex(sum) {
					error!(n, filename, sum, "chunk sum not in right format: {err}");
					return false;
				}

				true
			}),
	)
	.await
	.iter()
	.all(|t| *t)
}

fn chunk_readers(
	input: &Path,
	meta: &ChunkedMetadata,
) -> impl Stream<Item = std::io::Result<ChunkReader>> {
	let chunks: Vec<(PathBuf, Hash, u64)> = meta
		.chunks
		.iter()
		.enumerate()
		.map(|(n, (filename, sum))| {
			(
				input.join(filename),
				// UNWRAPs: prefix and hash were checked before
				Hash::from_hex(sum.strip_prefix("b3:").unwrap()).unwrap(),
				if n as u64 == meta.chunk_n {
					meta.full_size
						.saturating_sub(meta.chunk_size * (meta.chunk_n - 1))
				} else {
					meta.chunk_size
				},
			)
		})
		.collect();
	// collect: to not carry the input/meta lifetimes into the stream

	stream::iter(chunks.into_iter().map(|(path, sum, size)| {
		Box::pin(async move {
			File::open(path).await.map(|file| ChunkReader {
				file: ReaderStream::new(file),
				hasher: Hasher::new(),
				sum,
				size,
				read: 0,
			})
		})
		.into_stream()
	}))
	.flatten()
}

#[derive(Debug)]
struct ChunkReader {
	file: ReaderStream<File>,
	hasher: Hasher,
	sum: Hash,
	size: u64,
	read: u64,
}

impl Stream for ChunkReader {
	type Item = std::io::Result<Bytes>;

	fn size_hint(&self) -> (usize, Option<usize>) {
		let n = usize::try_from(self.size.saturating_sub(self.read));
		(n.unwrap_or(0), n.ok())
	}

	fn poll_next(
		mut self: Pin<&mut Self>,
		cx: &mut futures::task::Context<'_>,
	) -> Poll<Option<Self::Item>> {
		match self.file.poll_next_unpin(cx) {
			p @ Poll::Pending | p @ Poll::Ready(Some(Err(_))) => p,
			Poll::Ready(Some(Ok(bytes))) => {
				self.read += bytes.len() as u64;
				self.hasher.update(&bytes);
				Poll::Ready(Some(Ok(bytes)))
			}
			Poll::Ready(None) => {
				// chunk finished
				let sum = self.hasher.finalize();
				if self.sum != sum {
					Poll::Ready(Some(Err(std::io::Error::new(
						ErrorKind::InvalidData,
						format!(
							"chunk checksum mismatch!\nexpected: {}\nobtained: {sum}",
							self.sum
						),
					))))
				} else {
					Poll::Ready(None)
				}
			}
		}
	}
}
