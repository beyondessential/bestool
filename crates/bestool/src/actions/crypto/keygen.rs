use std::path::PathBuf;

use age::{secrecy::ExposeSecret, x25519};
use clap::Parser;
use miette::{IntoDiagnostic as _, Result};
use tracing::info;

use crate::actions::{crypto::CryptoArgs, Context};

/// Generate a key-pair to use in the "encrypt" and "decrypt" subcommands.
#[derive(Debug, Clone, Parser)]
pub struct KeygenArgs {
	/// The destination directory the output will be written to.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--output PATH`"))]
	#[arg(long, default_value = r"./")]
	pub output: PathBuf,
}

pub async fn run(ctx: Context<CryptoArgs, KeygenArgs>) -> Result<()> {
	let output = ctx.args_sub.output;
	let secret = x25519::Identity::generate();
	let public = secret.to_public();

	tokio::fs::write(
		output.join("private_key.txt"),
		secret.to_string().expose_secret().as_bytes(),
	)
	.await
	.into_diagnostic()?;

	tokio::fs::write(output.join("public_key.txt"), public.to_string().as_bytes())
		.await
		.into_diagnostic()?;

	info!(
		?output,
		"the generated key written as 'public_key.txt' and 'private_key.txt'"
	);

	Ok(())
}
