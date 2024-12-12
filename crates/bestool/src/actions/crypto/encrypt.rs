use std::{iter, path::PathBuf};

use age::{x25519, Encryptor};
use clap::Parser;
use miette::{miette, Context as _, IntoDiagnostic as _, Result};
use tokio::io::AsyncWriteExt;
use tokio_util::compat::{FuturesAsyncWriteCompatExt as _, TokioAsyncWriteCompatExt as _};

use crate::actions::{crypto::CryptoArgs, Context};

#[derive(Debug, Clone, Parser)]
pub struct EncryptArgs {
	plaintext: PathBuf,

	#[arg(long)]
	public_key: PathBuf,
}

pub async fn run(ctx: Context<CryptoArgs, EncryptArgs>) -> Result<()> {
	let public_key: x25519::Recipient = tokio::fs::read_to_string(&ctx.args_sub.public_key)
		.await
		.into_diagnostic()
		.wrap_err("reading the public key")?
		.parse()
		.map_err(|err: &str| miette!("failed to parse: {err}"))?;

	let mut plaintext = tokio::fs::File::open(&ctx.args_sub.plaintext)
		.await
		.into_diagnostic()
		.wrap_err("opening the plainetxt")?;

	let mut encrypted_path = ctx.args_sub.plaintext.into_os_string();
	encrypted_path.push(".enc");
	let encrypted = tokio::fs::File::create_new(encrypted_path)
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

	Ok(())
}
