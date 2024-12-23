use std::{fmt::Debug, path::PathBuf};

use clap::Parser;
use miette::Result;

use super::{keys::KeyArgs, streams::encrypt_file, CryptoArgs};
use crate::actions::Context;

/// Encrypt a file using a public key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
///
/// If symmetric cryptography (using a passphrase), see `protect`/`reveal`.
#[derive(Debug, Clone, Parser)]
pub struct EncryptArgs {
	/// File to be encrypted.
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	pub input: PathBuf,

	/// Path or filename to write the encrypted file to.
	///
	/// By default this is the input file, with `.age` appended.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-o, --output PATH`"))]
	#[arg(short, long)]
	pub output: Option<PathBuf>,

	#[command(flatten)]
	pub key: KeyArgs,
}

pub async fn run(ctx: Context<CryptoArgs, EncryptArgs>) -> Result<()> {
	let EncryptArgs {
		input: ref plaintext_path,
		output,
		key,
	} = ctx.args_sub;

	let public_key = key.require_public_key().await?;
	let encrypted_path = if let Some(path) = output {
		path
	} else {
		let mut path = plaintext_path.clone().into_os_string();
		path.push(".age");
		path.into()
	};

	encrypt_file(plaintext_path, encrypted_path, public_key).await?;
	Ok(())
}
