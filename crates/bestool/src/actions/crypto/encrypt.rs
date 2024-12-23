use std::{iter, path::PathBuf};

use age::{Encryptor, Recipient};
use clap::Parser;
use miette::{IntoDiagnostic as _, Result, WrapErr as _};
use tokio::{fs::File, io::AsyncWriteExt as _};
use tokio_util::compat::{FuturesAsyncWriteCompatExt as _, TokioAsyncWriteCompatExt as _};
use tracing::{debug, trace};

use super::{key::KeyArgs, with_progress_bar, CryptoArgs};
use crate::actions::Context;

/// Encrypt a file using a public key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
#[derive(Debug, Clone, Parser)]
pub struct EncryptArgs {
	/// File to be encrypted.
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	input: PathBuf,

	/// Path or filename to write the encrypted file to.
	///
	/// By default this is the input file, with `.age` appended.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-o, --output PATH`"))]
	#[arg(short, long)]
	output: Option<PathBuf>,

	#[command(flatten)]
	key: KeyArgs,
}

pub async fn run(ctx: Context<CryptoArgs, EncryptArgs>) -> Result<()> {
	let EncryptArgs {
		input: ref plaintext_path,
		output,
		key,
	} = ctx.args_sub;

	let public_key = key.require_public_key().await?;
	let encrypted_path = if let Some(path) = output {
		path
	} else {
		let mut path = plaintext_path.clone().into_os_string();
		path.push(".age");
		path.into()
	};

	debug!(
		input=?plaintext_path,
		output=?encrypted_path,
		"encrypting"
	);

	let plaintext = File::open(&plaintext_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the plainetxt")?;
	let plaintext_length = plaintext
		.metadata()
		.await
		.into_diagnostic()
		.wrap_err("reading input file length")?
		.len();

	let encrypted = File::create_new(&encrypted_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the encrypted output")?;

	encrypt_stream(
		with_progress_bar(plaintext_length, plaintext),
		encrypted.compat_write(),
		public_key,
	)
	.await?;

	Ok(())
}

/// Encrypt a bytestream given a public key.
pub(crate) async fn encrypt_stream<
	R: tokio::io::AsyncRead + Unpin,
	W: futures::AsyncWrite + Unpin,
>(
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
