use std::{
	fmt::Debug,
	io::{stderr, IsTerminal as _},
	path::{Path, PathBuf},
};

use age::{Identity, Recipient};
use indicatif::{ProgressBar, ProgressBarIter, ProgressStyle};
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::{fs::File, io::AsyncRead};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};
use tracing::instrument;

use crate::streams::{decrypt_stream, encrypt_stream};

/// Wraps a [`tokio::io::AsyncRead`] with an [`indicatif::ProgressBar`].
///
/// The progress bar outputs to stderr iff that's terminal, and nothing is displayed otherwise.
pub fn with_progress_bar<R: AsyncRead + Unpin>(
	expected_length: u64,
	reader: R,
) -> ProgressBarIter<R> {
	if stderr().is_terminal() {
		let style = ProgressStyle::default_bar()
			.template("[{bar:.green/blue}] {wide_msg} {binary_bytes}/{binary_total_bytes} ({eta})")
			.expect("BUG: progress bar template invalid");
		ProgressBar::new(expected_length).with_style(style)
	} else {
		ProgressBar::hidden()
	}
	.wrap_async_read(reader)
}

/// Encrypt a path to another given a [`Recipient`].
///
/// If stderr is a terminal, this will show a progress bar.
#[instrument(level = "debug", skip(key))]
pub async fn encrypt_file(
	input_path: impl AsRef<Path> + Debug,
	output_path: impl AsRef<Path> + Debug,
	key: Box<dyn Recipient + Send>,
) -> Result<u64> {
	let input = File::open(input_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the plainetxt")?;
	let input_length = input
		.metadata()
		.await
		.into_diagnostic()
		.wrap_err("reading input file length")?
		.len();

	let output = File::create_new(output_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the encrypted output")?;

	encrypt_stream(
		with_progress_bar(input_length, input),
		output.compat_write(),
		key,
	)
	.await
}

/// Decrypt a path to another given an [`Identity`].
///
/// If stderr is a terminal, this will show a progress bar.
#[instrument(level = "debug", skip(key))]
pub async fn decrypt_file(
	input_path: impl AsRef<Path> + Debug,
	output_path: impl AsRef<Path> + Debug,
	key: Box<dyn Identity>,
) -> Result<u64> {
	let input = File::open(input_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the input file")?;
	let input_length = input
		.metadata()
		.await
		.into_diagnostic()
		.wrap_err("reading input file length")?
		.len();

	let output = File::create_new(output_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the output file")?;

	decrypt_stream(with_progress_bar(input_length, input).compat(), output, key).await
}

/// Append `.age` to a file path.
pub fn append_age_ext(path: impl AsRef<Path>) -> PathBuf {
	let mut path = path.as_ref().as_os_str().to_owned();
	path.push(".age");
	path.into()
}

/// Remove the `.age` suffix from a file path, if present.
///
/// Returns `Err(original path)` if the suffix isn't present.
pub fn remove_age_ext<T: AsRef<Path>>(path: T) -> std::result::Result<PathBuf, T> {
	if !path.as_ref().extension().is_some_and(|ext| ext == "age") {
		Err(path)
	} else {
		Ok(path.as_ref().with_extension(""))
	}
}
