use std::{fmt::Debug, path::PathBuf};

use clap::Parser;
use miette::{bail, Result};

use super::{keys::PassphraseArgs, streams::decrypt_file, CryptoArgs};
use crate::actions::Context;

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
	pub key: PassphraseArgs,
}

pub async fn run(ctx: Context<CryptoArgs, RevealArgs>) -> Result<()> {
	let RevealArgs {
		ref input,
		output,
		key,
	} = ctx.args_sub;

	let key = key.require().await?;
	let output = if let Some(ref path) = output {
		path.to_owned()
	} else {
		if !input.extension().is_some_and(|ext| ext == "age") {
			bail!("Cannot guess output path, use --output to set one");
		}
		input.with_extension("")
	};

	decrypt_file(input, output, Box::new(key)).await?;
	Ok(())
}
