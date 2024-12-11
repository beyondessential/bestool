use std::path::PathBuf;

use age::x25519;
use clap::Parser;
use miette::{miette, Context as _, IntoDiagnostic as _, Result};

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

	let encrypted = tokio::fs::read(&ctx.args_sub.encrypted)
		.await
		.into_diagnostic()
		.wrap_err("reading the encrypted file")?;

	let plaintext = age::decrypt(&private_key, &encrypted).into_diagnostic()?;
	tokio::fs::write(ctx.args_sub.encrypted.with_extension(""), plaintext)
		.await
		.into_diagnostic()
		.wrap_err("writing the decrypted data")?;

	Ok(())
}
