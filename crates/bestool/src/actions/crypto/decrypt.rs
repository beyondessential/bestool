use std::{fmt::Debug, path::PathBuf};

use clap::Parser;
use miette::{bail, Result};

use super::{keys::KeyArgs, streams::decrypt_file, CryptoArgs};
use crate::actions::Context;

/// Decrypt a file using a private key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
#[derive(Debug, Clone, Parser)]
pub struct DecryptArgs {
	/// File to be decrypted.
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	pub input: PathBuf,

	/// Path or filename to write the decrypted file to.
	///
	/// If the input file has a `.age` extension, this can be automatically derived (by removing the
	/// `.age`). Otherwise, this option is required.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-o, --output PATH`"))]
	#[arg(short, long)]
	pub output: Option<PathBuf>,

	#[command(flatten)]
	pub key: KeyArgs,
}

pub async fn run(ctx: Context<CryptoArgs, DecryptArgs>) -> Result<()> {
	let DecryptArgs {
		input: ref encrypted_path,
		output,
		key,
	} = ctx.args_sub;

	let secret_key = key.require_secret_key().await?;
	let plaintext_path = if let Some(ref path) = output {
		path.to_owned()
	} else {
		if !encrypted_path.extension().is_some_and(|ext| ext == "age") {
			bail!("Unknown file extension (expected .age): failed to derive the output file name.");
		}
		encrypted_path.with_extension("")
	};

	decrypt_file(encrypted_path, plaintext_path, secret_key).await?;
	Ok(())
}
