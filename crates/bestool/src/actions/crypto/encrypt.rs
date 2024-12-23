use std::{iter, path::PathBuf};

use age::Encryptor;
use clap::Parser;
use miette::{IntoDiagnostic as _, Result, WrapErr as _};
use tokio::{fs::File, io::AsyncWriteExt as _};
use tokio_util::compat::{FuturesAsyncWriteCompatExt as _, TokioAsyncWriteCompatExt as _};
use tracing::info;

use super::{key::KeyArgs, wrap_async_read_with_progress_bar, CryptoArgs};
use crate::actions::Context;

/// Encrypt a file using a public key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
#[derive(Debug, Clone, Parser)]
pub struct EncryptArgs {
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	input: PathBuf,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-o, --output PATH`"))]
	#[arg(short, long)]
	output: Option<PathBuf>,

	#[command(flatten)]
	key: KeyArgs,
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

	info!(
		input=?plaintext_path,
		output=?encrypted_path,
		"encrypting"
	);

	let plaintext = File::open(&plaintext_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the plainetxt")?;
	// Wrap with progress bar before introducing "age" to avoid predicting size after encryption.
	let mut plaintext = wrap_async_read_with_progress_bar(plaintext).await?;

	let encrypted = File::create_new(&encrypted_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the encrypted output")?;

	let mut encrypting_writer = Encryptor::with_recipients(iter::once(&*public_key as _))
		.expect("a recipient should exist")
		.wrap_async_output(encrypted.compat_write())
		.await
		.into_diagnostic()?
		.compat_write();

	tokio::io::copy(&mut plaintext, &mut encrypting_writer)
		.await
		.into_diagnostic()
		.wrap_err("encrypting data in stream")?;

	encrypting_writer
		.shutdown()
		.await
		.into_diagnostic()
		.wrap_err("closing the encrypted output")?;

	info!("finished encrypting");
	Ok(())
}
