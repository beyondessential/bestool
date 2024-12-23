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
/// This creates a single identity file, compatible with `age-keygen`, which
/// contains both the public and secret key:
///
/// ```identity.txt
/// # created: 2024-12-20T05:36:10.267871872+00:00
/// # public key: age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
/// AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
/// ```
///
/// Do NOT store this entire identity file on untrusted machines, or where it's
/// unnecessary to regularly decrypt files. Instead, copy just the public key
/// string, which can safely be shared and stored as-is:
///
/// ```key.pub
/// age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
/// ```
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct KeygenArgs {
	/// Path to write the identity file to.
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
