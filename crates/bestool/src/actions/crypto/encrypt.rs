use std::path::PathBuf;

use age::x25519;
use clap::Parser;
use miette::{miette, Context as _, IntoDiagnostic as _, Result};

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

	let plaintext = tokio::fs::read(&ctx.args_sub.plaintext)
		.await
		.into_diagnostic()
		.wrap_err("reading the plainetxt")?;
	let encrypted = age::encrypt(&public_key, &plaintext).into_diagnostic()?;

	let mut encrypted_path = ctx.args_sub.plaintext.into_os_string();
	encrypted_path.push(".enc");
	tokio::fs::write(encrypted_path, encrypted)
		.await
		.into_diagnostic()
		.wrap_err("writing the encrypted data")?;

	Ok(())
}
