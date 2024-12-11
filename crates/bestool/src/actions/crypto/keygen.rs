use std::path::PathBuf;

use age::{secrecy::ExposeSecret, x25519};
use clap::Parser;
use miette::{IntoDiagnostic, Result};

use crate::actions::{crypto::CryptoArgs, Context};

#[derive(Debug, Clone, Parser)]
pub struct KeygenArgs {
	/// The destination directory the output will be written to.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--write-to PATH`"))]
	#[arg(long, default_value = r"./")]
	pub output: PathBuf,
}

pub async fn run(ctx: Context<CryptoArgs, KeygenArgs>) -> Result<()> {
	let output = ctx.args_sub.output;
	let secret = x25519::Identity::generate();
	let public = secret.to_public();

	tokio::fs::write(
		output.join("secret_key.txt"),
		secret.to_string().expose_secret().as_bytes(),
	)
	.await
	.into_diagnostic()?;

	tokio::fs::write(output.join("public_key.txt"), public.to_string().as_bytes())
		.await
		.into_diagnostic()?;

	Ok(())
}
