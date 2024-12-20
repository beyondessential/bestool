use std::{iter, path::PathBuf};

use age::{x25519, IdentityFile, Recipient, Encryptor};
use clap::Parser;
use miette::{bail, miette, WrapErr as _, IntoDiagnostic as _, Result};
use tokio::{fs::{read_to_string, File}, io::AsyncWriteExt as _};
use tokio_util::compat::{FuturesAsyncWriteCompatExt as _, TokioAsyncWriteCompatExt as _};
use tracing::info;

use crate::actions::{
	crypto::{wrap_async_read_with_progress_bar, CryptoArgs},
	Context,
};

/// Encrypt a file using a public key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
///
///
#[derive(Debug, Clone, Parser)]
pub struct EncryptArgs {
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	input: PathBuf,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-o, --output PATH`"))]
	#[arg(short, long)]
	output: Option<PathBuf>,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-k, --key-path PATH`"))]
	#[arg(short = 'k', long = "key-path")]
	public_key_path: Option<PathBuf>,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-K, --key KEY`"))]
	#[arg(short = 'K', long = "key")]
	public_key: Option<String>,
}

pub async fn run(ctx: Context<CryptoArgs, EncryptArgs>) -> Result<()> {
	let EncryptArgs {
		input: ref plaintext_path,
		..
	} = ctx.args_sub;

	let public_key: Box<dyn Recipient + Send> = match ctx.args_sub {
		EncryptArgs { public_key_path: None, public_key: None, .. } => {
			bail!("one of `--key-path` or `--key` must be provided");
		}
		EncryptArgs { public_key_path: Some(_), public_key: Some(_), .. } => {
			bail!("one of `--key-path` or `--key` must be provided, not both");
		}
		EncryptArgs { public_key: Some(key), .. } => {
			Box::new(
				key.parse::<x25519::Recipient>()
					.map_err(|err| miette!("{err}").wrap_err("parsing key"))?
			)
		}
		EncryptArgs { public_key_path: Some(path), .. } => {
			let key = read_to_string(&path).await.into_diagnostic().wrap_err("reading keyfile")?;
			if key.starts_with("age") {
				Box::new(
					key.parse::<x25519::Recipient>()
						.map_err(|err| miette!("{err}").wrap_err("parsing key"))?
				)
			} else {
				let recipient = IdentityFile::from_buffer(key.as_bytes())
					.into_diagnostic()
					.wrap_err("parsing identity")?
					.to_recipients()
					.into_diagnostic()
					.wrap_err("parsing recipients from identity")?
					.pop()
					.ok_or_else(|| miette!("no recipient available in identity"))?;
				recipient
			}
		}
	};

	let encrypted_path = if let Some(path) = ctx.args_sub.output { path } else {
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
