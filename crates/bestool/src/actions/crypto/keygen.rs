use std::path::PathBuf;

use age::{secrecy::ExposeSecret, x25519};
use chrono::Utc;
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::{fs::File, io::AsyncWriteExt};
use tracing::info;

use crate::{
	actions::{crypto::CryptoArgs, Context},
	now_time,
};

/// Generate a key-pair to use in the "encrypt" and "decrypt" subcommands.
///
/// This makes a single identity file as in `age-keygen --output PATH`
#[derive(Debug, Clone, Parser)]
pub struct KeygenArgs {
	/// Path to the output identity key file.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--identity PATH`"))]
	#[arg(long, default_value = r"identity.txt")]
	pub identity: PathBuf,
}

pub async fn run(ctx: Context<CryptoArgs, KeygenArgs>) -> Result<()> {
	let identity_path = ctx.args_sub.identity;
	let secret = x25519::Identity::generate();
	let public = secret.to_public();

	File::create_new(&identity_path)
		.await
		.into_diagnostic()
		.wrap_err("opening the identity file")?
		.write_all(
			format!(
				"# created: {}\n# public key: {}\n{}\n",
				now_time(&Utc).to_rfc3339(),
				public.to_string(),
				secret.to_string().expose_secret()
			)
			.as_bytes(),
		)
		.await
		.into_diagnostic()
		.wrap_err("writing the identity")?;

	info!(?identity_path, "wrote the generated key to");

	Ok(())
}
