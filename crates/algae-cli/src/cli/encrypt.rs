use std::{fmt::Debug, path::PathBuf};

use clap::Parser;
use miette::{IntoDiagnostic, Result, WrapErr};
use tokio::fs::remove_file;

use crate::{
	files::{append_age_ext, encrypt_file},
	keys::KeyArgs,
};

/// Encrypt a file using a public key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
///
/// For symmetric cryptography (using a passphrase), see `protect`/`reveal`.
#[derive(Debug, Clone, Parser)]
pub struct EncryptArgs {
	/// File to be encrypted.
	pub input: PathBuf,

	/// Path or filename to write the encrypted file to.
	///
	/// By default this is the input file, with `.age` appended.
	#[arg(short, long)]
	pub output: Option<PathBuf>,

	/// Delete input file after encrypting.
	#[arg(long = "rm")]
	pub remove: bool,

	#[command(flatten)]
	#[allow(missing_docs, reason = "don't interfere with clap")]
	pub key: KeyArgs,
}

/// CLI command for the `encrypt` operation (public key encryption).
pub async fn run(
	EncryptArgs {
		ref input,
		output,
		key,
		remove,
	}: EncryptArgs,
) -> Result<()> {
	let public_key = key.require_public_key().await?;
	let output = output.unwrap_or_else(|| append_age_ext(input));

	encrypt_file(input, output, public_key).await?;

	if remove {
		remove_file(input)
			.await
			.into_diagnostic()
			.wrap_err("deleting input file")?;
	}

	Ok(())
}
