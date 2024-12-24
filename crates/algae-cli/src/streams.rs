use std::iter;

use age::{Decryptor, Encryptor, Identity, Recipient};
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::io::AsyncWriteExt as _;
use tokio_util::compat::{FuturesAsyncReadCompatExt as _, FuturesAsyncWriteCompatExt as _};
use tracing::trace;

/// Encrypt a bytestream given a [`Recipient`].
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

/// Decrypt a bytestream given an [`Identity`].
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
