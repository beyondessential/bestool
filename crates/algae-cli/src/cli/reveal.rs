use std::{fmt::Debug, path::PathBuf};

use clap::Parser;
use miette::{miette, Result};

use crate::{
	files::{decrypt_file, remove_age_ext},
	passphrases::PassphraseArgs,
};

/// Decrypt a file using a passphrase.
///
/// Whenever possible, prefer to use `encrypt` and `decrypt` with identity files
/// (public key cryptography).
///
/// This utility may also be used to convert a passphrase-protected identity
/// file into a plaintext one.
#[derive(Debug, Clone, Parser)]
pub struct RevealArgs {
	/// File to be decrypted.
	pub input: PathBuf,

	/// Path or filename to write the decrypted file to.
	///
	/// If the input file has a `.age` extension, this can be automatically derived (by removing the
	/// `.age`). Otherwise, this option is required.
	#[arg(short, long)]
	pub output: Option<PathBuf>,

	#[command(flatten)]
	#[allow(missing_docs, reason = "don't interfere with clap")]
	pub key: PassphraseArgs,
}

/// CLI command for the `reveal` operation (passphrase decryption).
pub async fn run(
	RevealArgs {
		ref input,
		output,
		key,
	}: RevealArgs,
) -> Result<()> {
	let key = key.require().await?;
	let output = if let Some(ref path) = output {
		path.to_owned()
	} else {
		remove_age_ext(input)
			.map_err(|_| miette!("Cannot guess output path, use --output to set one"))?
	};

	decrypt_file(input, output, Box::new(key)).await?;
	Ok(())
}
