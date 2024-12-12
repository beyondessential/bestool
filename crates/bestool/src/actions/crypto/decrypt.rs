use std::{iter, path::PathBuf};

use age::{x25519, Decryptor};
use clap::Parser;
use miette::{miette, Context as _, IntoDiagnostic as _, Result};
use tokio::io::AsyncWriteExt;
use tokio_util::compat::{FuturesAsyncReadCompatExt as _, TokioAsyncReadCompatExt as _};

use crate::actions::{crypto::CryptoArgs, Context};

#[derive(Debug, Clone, Parser)]
pub struct DecryptArgs {
	encrypted: PathBuf,

	#[arg(long)]
	private_key: PathBuf,
}

pub async fn run(ctx: Context<CryptoArgs, DecryptArgs>) -> Result<()> {
	let private_key: x25519::Identity = tokio::fs::read_to_string(&ctx.args_sub.private_key)
		.await
		.into_diagnostic()
		.wrap_err("reading the private key")?
		.parse()
		.map_err(|err: &str| miette!("failed to parse: {err}"))?;

	let encrypted = tokio::fs::File::open(&ctx.args_sub.encrypted)
		.await
		.into_diagnostic()
		.wrap_err("opening the encrypted file")?;

	let mut plaintext = tokio::fs::File::create_new(ctx.args_sub.encrypted.with_extension(""))
		.await
		.into_diagnostic()
		.wrap_err("opening the decrypted output")?;

	let mut decrypting_reader = Decryptor::new_async(encrypted.compat())
		.await
		.into_diagnostic()?
		.decrypt_async(iter::once(&private_key as _))
		.into_diagnostic()?
		.compat();

	tokio::io::copy(&mut decrypting_reader, &mut plaintext)
		.await
		.into_diagnostic()
		.wrap_err("decrypting data in stream")?;

	plaintext
		.shutdown()
		.await
		.into_diagnostic()
		.wrap_err("closing the plaintext output")?;
	Ok(())
}
