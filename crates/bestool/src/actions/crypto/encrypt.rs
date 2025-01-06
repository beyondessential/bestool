use std::path::PathBuf;

use age::x25519;
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::fs::File;
use tracing::info;

use crate::actions::{
	crypto::{self, copy_encrypting, wrap_async_read_with_progress_bar, CryptoArgs},
	Context,
};

#[derive(Debug, Clone, Parser)]
pub struct EncryptArgs {
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `PATH`"))]
	plaintext: PathBuf,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--public-key PATH`"))]
	#[arg(long, group = "key", required = true)]
	public_key: Option<PathBuf>,

	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--private-key PATH`"))]
	#[arg(long, group = "key", required = true)]
	private_key: Option<PathBuf>,
}

pub async fn run(ctx: Context<CryptoArgs, EncryptArgs>) -> Result<()> {
	let EncryptArgs {
		plaintext: plaintext_path,
		public_key: public_key_path_opt,
		private_key: private_key_path_opt,
	} = ctx.args_sub;
	let mut encrypted_path = plaintext_path.clone().into_os_string();
	encrypted_path.push(".age");
	info!(
		?plaintext_path,
		?encrypted_path,
		?public_key_path_opt,
		?private_key_path_opt,
		"encrypting"
	);

	let public_key = if let Some(public_key_path) = public_key_path_opt {
		crypto::read_age_key::<x25519::Recipient>(&public_key_path).await?
	} else if let Some(private_key_path) = private_key_path_opt {
		crypto::read_age_key::<x25519::Identity>(&private_key_path)
			.await?
			.to_public()
	} else {
		unreachable!()
	};

	let plaintext = File::open(&plaintext_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the plainetxt")?;
	// Wrap with progress bar before introducing "age" to avoid predicting size after encryption.
	let mut plaintext = wrap_async_read_with_progress_bar(plaintext).await?;

	let mut encrypted = File::create_new(&encrypted_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the encrypted output")?;

	copy_encrypting(&mut plaintext, &mut encrypted, &public_key).await?;

	info!("finished encrypting");
	Ok(())
}
