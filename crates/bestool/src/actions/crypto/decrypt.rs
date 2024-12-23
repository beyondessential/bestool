use std::{iter, path::PathBuf};

use age::{Decryptor, Identity};
use clap::Parser;
use miette::{bail, Context as _, IntoDiagnostic as _, Result};
use tokio::{fs::File, io::AsyncWriteExt as _};
use tokio_util::compat::{FuturesAsyncReadCompatExt as _, TokioAsyncReadCompatExt as _};
use tracing::{debug, trace};

use super::{key::KeyArgs, with_progress_bar, CryptoArgs};
use crate::actions::Context;

/// Decrypt a file using a private key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
#[derive(Debug, Clone, Parser)]
pub struct DecryptArgs {
	/// File to be decrypted.
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	input: PathBuf,

	/// Path or filename to write the decrypted file to.
	///
	/// If the input file has a `.age` extension, this can be automatically derived (by removing the
	/// `.age`). Otherwise, this option is required.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-o, --output PATH`"))]
	#[arg(short, long)]
	output: Option<PathBuf>,

	#[command(flatten)]
	key: KeyArgs,
}

pub async fn run(ctx: Context<CryptoArgs, DecryptArgs>) -> Result<()> {
	let DecryptArgs {
		input: ref encrypted_path,
		output,
		key,
	} = ctx.args_sub;

	let secret_key = key.require_secret_key().await?;
	let plaintext_path = if let Some(ref path) = output {
		path.to_owned()
	} else {
		if !encrypted_path.extension().is_some_and(|ext| ext == "age") {
			bail!("Unknown file extension (expected .age): failed to derive the output file name.");
		}
		encrypted_path.with_extension("")
	};

	debug!(
		input=?encrypted_path,
		output=?plaintext_path,
		"decrypting"
	);

	let input = File::open(&encrypted_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the input file")?;
	let input_length = input
		.metadata()
		.await
		.into_diagnostic()
		.wrap_err("reading input file length")?
		.len();

	let output = File::create_new(&plaintext_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the output file")?;

	decrypt_stream(
		with_progress_bar(input_length, input).compat(),
		output,
		secret_key,
	)
	.await?;

	Ok(())
}

/// Decrypt a bytestream given a secret key.
pub(crate) async fn decrypt_stream<
	R: futures::AsyncRead + Unpin,
	W: tokio::io::AsyncWrite + Unpin,
>(
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
