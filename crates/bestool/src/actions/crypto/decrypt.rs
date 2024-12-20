use std::{iter, path::PathBuf};

use age::{x25519, Decryptor};
use clap::Parser;
use miette::{bail, miette, Context as _, IntoDiagnostic as _, Result};
use tokio::{fs::File, io::AsyncWriteExt as _};
use tokio_util::compat::{FuturesAsyncReadCompatExt as _, TokioAsyncReadCompatExt as _};
use tracing::info;

use crate::actions::{
	crypto::{wrap_async_read_with_progress_bar, CryptoArgs},
	Context,
};

/// Decrypt the file encrypted by the "encrypt" subcommand.
#[derive(Debug, Clone, Parser)]
pub struct DecryptArgs {
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `INPUT PATH`"))]
	file: PathBuf,

	#[cfg_attr(docsrs, doc("\n\n**Argument**: `[OUTPUT PATH]`"))]
	output: Option<PathBuf>,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--private-key PATH`"))]
	#[arg(long)]
	private_key: PathBuf,
}

pub async fn run(ctx: Context<CryptoArgs, DecryptArgs>) -> Result<()> {
	let DecryptArgs {
		file: encrypted_path,
		private_key: private_key_path,
		..
	} = ctx.args_sub;

	let plaintext_path = if let Some(path) = ctx.args_sub.output { path } else {
		if !encrypted_path.extension().is_some_and(|ext| ext == "age") {
			bail!("Unknown file extension (expected .age): failed to derive the output file name.");
		}
		encrypted_path.with_extension("")
	};
	info!(
		?encrypted_path,
		?plaintext_path,
		?private_key_path,
		"decrypting"
	);

	let private_key: x25519::Identity = tokio::fs::read_to_string(&private_key_path)
		.await
		.into_diagnostic()
		.wrap_err("reading the private key")?
		.parse()
		.map_err(|err: &str| miette!("failed to parse: {err}"))?;

	let encrypted = File::open(&encrypted_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the encrypted file")?;

	// Progress is calculated on the input size, not the predicted output
	let encrypted = wrap_async_read_with_progress_bar(encrypted).await?;

	let mut plaintext = File::create_new(&plaintext_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the output file")?;

	let mut decrypting_reader = Decryptor::new_async(encrypted.compat())
		.await
		.into_diagnostic()?
		.decrypt_async(iter::once(&private_key as _))
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
