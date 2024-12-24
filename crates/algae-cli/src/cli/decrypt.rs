use std::{fmt::Debug, path::PathBuf};

use clap::Parser;
use miette::{miette, Result};

use crate::{
	files::{decrypt_file, remove_age_ext},
	keys::KeyArgs,
};

/// Decrypt a file using a secret key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
///
/// For symmetric cryptography (using a passphrase), see `protect`/`reveal`.
#[derive(Debug, Clone, Parser)]
pub struct DecryptArgs {
	/// File to be decrypted.
	pub input: PathBuf,

	/// Path or filename to write the decrypted file to.
	///
	/// If the input file has a `.age` extension, this can be automatically
	/// derived (by removing the `.age`). Otherwise, this option is required.
	#[arg(short, long)]
	pub output: Option<PathBuf>,

	#[command(flatten)]
	#[allow(missing_docs, reason = "don't interfere with clap")]
	pub key: KeyArgs,
}

/// CLI command for the `decrypt` operation (secret key decryption).
pub async fn run(
	DecryptArgs {
		ref input,
		output,
		key,
	}: DecryptArgs,
) -> Result<()> {
	let secret_key = key.require_secret_key().await?;
	let output = if let Some(ref path) = output {
		path.to_owned()
	} else {
		remove_age_ext(input)
			.map_err(|_| miette!("Cannot guess output path, use --output to set one"))?
	};

	decrypt_file(input, output, secret_key).await?;
	Ok(())
}
