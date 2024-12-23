use std::{fmt::Debug, path::PathBuf};

use clap::Parser;
use miette::{IntoDiagnostic, Result, WrapErr};
use tokio::fs::remove_file;

use super::{keys::PassphraseArgs, streams::encrypt_file, CryptoArgs};
use crate::actions::Context;

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
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	pub input: PathBuf,

	/// Path or filename to write the encrypted file to.
	///
	/// By default this is the input file, with `.age` appended.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-o, --output PATH`"))]
	#[arg(short, long)]
	pub output: Option<PathBuf>,

	/// Delete input file after encrypting.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--rm`"))]
	#[arg(long = "rm")]
	pub remove: bool,

	#[command(flatten)]
	pub key: PassphraseArgs,
}

pub async fn run(ctx: Context<CryptoArgs, ProtectArgs>) -> Result<()> {
	let ProtectArgs {
		ref input,
		output,
		key,
		remove,
	} = ctx.args_sub;

	let key = key.require_with_confirmation().await?;
	let output = if let Some(path) = output {
		path
	} else {
		let mut path = input.clone().into_os_string();
		path.push(".age");
		path.into()
	};

	encrypt_file(input, output, Box::new(key)).await?;

	if remove {
		remove_file(input)
			.await
			.into_diagnostic()
			.wrap_err("deleting input file")?;
	}

	Ok(())
}
