use std::{iter, path::PathBuf};

use age::{x25519, Encryptor};
use clap::Parser;
use miette::{miette, Context as _, IntoDiagnostic as _, Result};
use tokio::{fs::File, io::AsyncWriteExt as _};
use tokio_util::compat::{FuturesAsyncWriteCompatExt as _, TokioAsyncWriteCompatExt as _};
use tracing::info;

use crate::actions::{
	crypto::{wrap_async_read_with_progress_bar, CryptoArgs},
	Context,
};

#[derive(Debug, Clone, Parser)]
pub struct EncryptArgs {
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	plaintext: PathBuf,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--public-key PATH`"))]
	#[arg(long)]
	public_key: PathBuf,
}

pub async fn run(ctx: Context<CryptoArgs, EncryptArgs>) -> Result<()> {
	let EncryptArgs {
		plaintext: plaintext_path,
		public_key: public_key_path,
	} = ctx.args_sub;
	let mut encrypted_path = plaintext_path.clone().into_os_string();
	encrypted_path.push(".enc");
	info!(
		?plaintext_path,
		?encrypted_path,
		?public_key_path,
		"encrypting"
	);

	let public_key: x25519::Recipient = tokio::fs::read_to_string(&public_key_path)
		.await
		.into_diagnostic()
		.wrap_err("reading the public key")?
		.parse()
		.map_err(|err: &str| miette!("failed to parse: {err}"))?;

	let plaintext = File::open(&plaintext_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the plainetxt")?;
	// Wrap with progress bar before introducing "age" to avoid predicting size after encryption.
	let mut plaintext = wrap_async_read_with_progress_bar(plaintext).await?;

	let encrypted = File::create_new(&encrypted_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the encrypted output")?;

	let mut encrypting_writer = Encryptor::with_recipients(iter::once(&public_key as _))
		.expect("a recipient should exist")
		.wrap_async_output(encrypted.compat_write())
		.await
		.into_diagnostic()?
		.compat_write();

	tokio::io::copy(&mut plaintext, &mut encrypting_writer)
		.await
		.into_diagnostic()
		.wrap_err("encrypting data in stream")?;

	encrypting_writer
		.shutdown()
		.await
		.into_diagnostic()
		.wrap_err("closing the encrypted output")?;

	info!("finished encrypting");
	Ok(())
}
