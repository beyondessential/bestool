use std::{
	fmt::Debug,
	io::{stderr, IsTerminal as _},
	iter,
	path::Path,
};

use age::{Decryptor, Encryptor, Identity, Recipient};
use indicatif::{ProgressBar, ProgressBarIter, ProgressStyle};
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::{
	fs::File,
	io::{AsyncRead, AsyncWriteExt as _},
};
use tokio_util::compat::{
	FuturesAsyncReadCompatExt as _, FuturesAsyncWriteCompatExt as _, TokioAsyncReadCompatExt as _,
	TokioAsyncWriteCompatExt as _,
};
use tracing::{instrument, trace};

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

/// Encrypt a bytestream given a public key.
pub async fn encrypt_stream<R: tokio::io::AsyncRead + Unpin, W: futures::AsyncWrite + Unpin>(
	mut reader: R,
	writer: W,
	key: Box<dyn Recipient + Send>,
) -> Result<u64> {
	let mut encrypting_writer = Encryptor::with_recipients(iter::once(&*key as _))
		.expect("BUG: a single recipient is always given")
		.wrap_async_output(writer)
		.await
		.into_diagnostic()?
		.compat_write();

	let bytes = tokio::io::copy(&mut reader, &mut encrypting_writer)
		.await
		.into_diagnostic()
		.wrap_err("encrypting data in stream")?;

	encrypting_writer
		.shutdown()
		.await
		.into_diagnostic()
		.wrap_err("closing the encrypted output")?;

	trace!(?bytes, "bytestream encrypted");

	Ok(bytes)
}

/// Encrypt a path to another given a public key.
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

/// Decrypt a bytestream given a secret key.
pub async fn decrypt_stream<R: futures::AsyncRead + Unpin, W: tokio::io::AsyncWrite + Unpin>(
	reader: R,
	mut writer: W,
	key: Box<dyn Identity>,
) -> Result<u64> {
	let mut decrypting_reader = Decryptor::new_async(reader)
		.await
		.into_diagnostic()?
		.decrypt_async(iter::once(&*key))
		.into_diagnostic()?
		.compat();

	let bytes = tokio::io::copy(&mut decrypting_reader, &mut writer)
		.await
		.into_diagnostic()
		.wrap_err("decrypting data")?;

	writer
		.shutdown()
		.await
		.into_diagnostic()
		.wrap_err("closing the output stream")?;

	trace!(?bytes, "bytestream decrypted");

	Ok(bytes)
}

/// Decrypt a path to another given a secret key.
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
