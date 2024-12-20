use std::{iter, path::PathBuf};

use age::{x25519, Decryptor, Identity, IdentityFile};
use clap::Parser;
use miette::{bail, miette, Context as _, IntoDiagnostic as _, Result};
use tokio::{fs::{read_to_string, File}, io::AsyncWriteExt as _};
use tokio_util::compat::{FuturesAsyncReadCompatExt as _, TokioAsyncReadCompatExt as _};
use tracing::info;

use crate::actions::{
	crypto::{wrap_async_read_with_progress_bar, CryptoArgs},
	Context,
};

/// Decrypt a file using a private key or an identity.
///
/// Either of `--key-path` or `--key` must be provided.
#[derive(Debug, Clone, Parser)]
pub struct DecryptArgs {
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	input: PathBuf,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-o, --output PATH`"))]
	#[arg(short, long)]
	output: Option<PathBuf>,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-k, --key-path PATH`"))]
	#[arg(short = 'k', long = "key-path")]
	private_key_path: Option<PathBuf>,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-K, --key STRING`"))]
	#[arg(short = 'K', long = "key")]
	private_key: Option<String>,
}

pub async fn run(ctx: Context<CryptoArgs, DecryptArgs>) -> Result<()> {
	let DecryptArgs { input: ref encrypted_path, .. } = ctx.args_sub;

	let plaintext_path = if let Some(ref path) = ctx.args_sub.output { path.to_owned() } else {
		if !encrypted_path.extension().is_some_and(|ext| ext == "age") {
			bail!("Unknown file extension (expected .age): failed to derive the output file name.");
		}
		encrypted_path.with_extension("")
	};

	let private_key: Box<dyn Identity> = match ctx.args_sub {
		DecryptArgs { private_key_path: None, private_key: None, .. } => {
			bail!("one of `--key-path` or `--key` must be provided");
		}
		DecryptArgs { private_key_path: Some(_), private_key: Some(_), .. } => {
			bail!("one of `--key-path` or `--key` must be provided, not both");
		}
		DecryptArgs { private_key: Some(key), .. } => {
			Box::new(
				key.parse::<x25519::Identity>()
					.map_err(|err| miette!("{err}").wrap_err("parsing key"))?
			)
		}
		DecryptArgs { private_key_path: Some(path), .. } => {
			let key = read_to_string(&path).await.into_diagnostic().wrap_err("reading keyfile")?;
			if key.starts_with("AGE-SECRET-KEY") {
				Box::new(
					key.parse::<x25519::Identity>()
						.map_err(|err| miette!("{err}").wrap_err("parsing key"))?
				)
			} else {
				IdentityFile::from_buffer(key.as_bytes())
					.into_diagnostic()
					.wrap_err("parsing identity")?
					.into_identities()
					.into_diagnostic()
					.wrap_err("parsing keys from identity")?
					.pop()
					.ok_or_else(|| miette!("no identity available"))?
			}
		}
	};

	info!(
		input=?encrypted_path,
		output=?plaintext_path,
		"decrypting"
	);

	let encrypted = File::open(&encrypted_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the input file")?;

	// Progress is calculated on the input size, not the predicted output
	let encrypted = wrap_async_read_with_progress_bar(encrypted).await?;

	let mut plaintext = File::create_new(&plaintext_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the output file")?;

	let mut decrypting_reader = Decryptor::new_async(encrypted.compat())
		.await
		.into_diagnostic()?
		.decrypt_async(iter::once(&*private_key))
		.into_diagnostic()?
		.compat();

	tokio::io::copy(&mut decrypting_reader, &mut plaintext)
		.await
		.into_diagnostic()
		.wrap_err("decrypting data")?;

	plaintext
		.shutdown()
		.await
		.into_diagnostic()
		.wrap_err("closing the output stream")?;

	info!("done");
	Ok(())
}
