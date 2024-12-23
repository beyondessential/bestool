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

/// Generate an identity (key pair) to encrypt and decrypt files
///
/// This creates a passphrase-protected identity file which contains both public
/// and secret keys:
///
/// ```identity.txt
/// # created: 2024-12-20T05:36:10.267871872+00:00
/// # public key: age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
/// AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
/// ```
///
/// As well as a plaintext public key file which contains just the public key:
///
/// ```identity.pub
/// age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
/// ```
///
/// The public key is also printed to stdout.
///
/// By default this command prompts for a passphrase. This can be disabled with
/// `--plaintext`; the default path `identity.txt` instead of `identity.txt.age`
/// is used if `--output` isn't given, and the contents will be in plain text
/// (in the format shown above).
///
/// On encrypting machines (e.g. servers uploading backups), you should always
/// prefer to store _just_ the public key, and only upload and use the
/// passphrase-protected identity file as necessary, deleting it afterwards.
///
/// Identity files (both plaintext and passphrase-protected) are compatible with
/// the `age` CLI tool. Note that the reverse might not be true.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct KeygenArgs {
	/// Path to write the identity file to.
	///
	/// Defaults to identity.txt.age, and to identity.txt if --plaintext is given.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-o, --output PATH`"))]
	#[arg(short, long)]
	pub output: Option<PathBuf>,

	/// Path to write the public key file to.
	///
	/// Set to a single hyphen (`-`) to disable writing this file; the public key
	/// will be printed to stdout in any case.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--public PATH`"))]
	#[arg(long = "public", default_value = "identity.pub")]
	pub public_path: PathBuf,

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
		public_path,
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

	println!("public key: {public}");
	if public_path.to_string_lossy() != "-" {
		File::create_new(&public_path)
			.await
			.into_diagnostic()
			.wrap_err("opening the public key file")?
			.write_all(public.to_string().as_bytes())
			.await
			.into_diagnostic()
			.wrap_err("writing the public key")?;
	}

	Ok(())
}
