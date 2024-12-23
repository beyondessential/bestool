use std::path::PathBuf;

use age::{
	secrecy::{ExposeSecret, SecretString},
	x25519,
};
use chrono::Utc;
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::{fs::File, io::AsyncWriteExt};

use crate::{
	actions::{crypto::CryptoArgs, Context},
	now_time,
};

use super::keys::PassphraseArgs;

/// Generate a key pair to encrypt and decrypt files with.
///
/// This creates a single identity file which contains a public and secret key:
///
/// ```identity.txt
/// # created: 2024-12-20T05:36:10.267871872+00:00
/// # public key: age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
/// AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
/// ```
///
/// By default this command prompts for a passphrase. This can be disabled with
/// `--plaintext`; the default path `identity.txt` instead of `identity.txt.age`
/// is used if `--output` isn't given, and the contents will be in plain text
/// (in the format shown above).
///
/// On encrypting machines (e.g. servers uploading backups), you should always
/// prefer to store _just_ the public key, in a separate file as below, and then
/// upload and use the passphrase-protected identity file as necessary, deleting
/// it afterwards.
///
/// ```key.pub
/// age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
/// ```
///
/// Identity files (both plaintext and passphrase-protected) are compatible with
/// the `age` CLI tool. Note that the reverse might not be true.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct KeygenArgs {
	/// Path to write the identity file to.
	///
	/// Defaults to identity.txt.age, and to identity.txt if --plaintext is given.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--output PATH`"))]
	#[arg(short, long)]
	pub output: Option<PathBuf>,

	/// INSECURE: write a plaintext identity.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--plaintext`"))]
	#[arg(long)]
	pub plaintext: bool,

	#[command(flatten)]
	pub key: PassphraseArgs,
}

pub async fn run(ctx: Context<CryptoArgs, KeygenArgs>) -> Result<()> {
	let KeygenArgs {
		output,
		plaintext,
		key,
	} = ctx.args_sub;

	let secret = x25519::Identity::generate();
	let public = secret.to_public();

	let output = output.unwrap_or_else(|| {
		if plaintext {
			"identity.txt"
		} else {
			"identity.txt.age"
		}
		.into()
	});

	let identity = SecretString::from(format!(
		"# created: {}\n# public key: {}\n{}\n",
		now_time(&Utc).to_rfc3339(),
		public.to_string(),
		secret.to_string().expose_secret()
	));

	let identity = if plaintext {
		identity.expose_secret().as_bytes().to_owned()
	} else {
		let key = key.require_with_confirmation().await?;
		age::encrypt(&key, identity.expose_secret().as_bytes()).into_diagnostic()?
	};

	File::create_new(&output)
		.await
		.into_diagnostic()
		.wrap_err("opening the identity file")?
		.write_all(&identity)
		.await
		.into_diagnostic()
		.wrap_err("writing the identity")?;

	Ok(())
}
