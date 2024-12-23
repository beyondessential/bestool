use std::{iter, path::PathBuf};

use age::Decryptor;
use clap::Parser;
use miette::{bail, Context as _, IntoDiagnostic as _, Result};
use tokio::{fs::File, io::AsyncWriteExt as _};
use tokio_util::compat::{FuturesAsyncReadCompatExt as _, TokioAsyncReadCompatExt as _};
use tracing::info;

use super::{key::KeyArgs, wrap_async_read_with_progress_bar, CryptoArgs};
use crate::actions::Context;

/// Decrypt a file using a private key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
#[derive(Debug, Clone, Parser)]
pub struct DecryptArgs {
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	input: PathBuf,

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

	info!(
		input=?encrypted_path,
		output=?plaintext_path,
		"decrypting"
	);

	let encrypted = File::open(&encrypted_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the input file")?;

	// Progress is calculated on the input size, not the predicted output
	let encrypted = wrap_async_read_with_progress_bar(encrypted).await?;

	let mut plaintext = File::create_new(&plaintext_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the output file")?;

	let mut decrypting_reader = Decryptor::new_async(encrypted.compat())
		.await
		.into_diagnostic()?
		.decrypt_async(iter::once(&*secret_key))
		.into_diagnostic()?
		.compat();

	tokio::io::copy(&mut decrypting_reader, &mut plaintext)
		.await
		.into_diagnostic()
		.wrap_err("decrypting data")?;

	plaintext
		.shutdown()
		.await
		.into_diagnostic()
		.wrap_err("closing the output stream")?;

	info!("done");
	Ok(())
}
