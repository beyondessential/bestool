use std::{fmt::Debug, path::PathBuf};

use clap::Parser;
use miette::{IntoDiagnostic, Result, WrapErr};
use tokio::fs::remove_file;

use crate::{
	files::{append_age_ext, encrypt_file},
	passphrases::PassphraseArgs,
};

/// Encrypt a file using a passphrase.
///
/// Whenever possible, prefer to use `encrypt` and `decrypt` with identity files
/// (public key cryptography).
///
/// This utility may also be used to convert a plaintext identity file into a
/// passphrase-protected one.
#[derive(Debug, Clone, Parser)]
pub struct ProtectArgs {
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
	pub key: PassphraseArgs,
}

/// CLI command for the `protect` operation (passphrase encryption).
pub async fn run(
	ProtectArgs {
		ref input,
		output,
		key,
		remove,
	}: ProtectArgs,
) -> Result<()> {
	let key = key.require_with_confirmation().await?;
	let output = output.unwrap_or_else(|| append_age_ext(input));

	encrypt_file(input, output, Box::new(key)).await?;

	if remove {
		remove_file(input)
			.await
			.into_diagnostic()
			.wrap_err("deleting input file")?;
	}

	Ok(())
}
