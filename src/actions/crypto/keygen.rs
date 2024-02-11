use std::{fs::File, path::PathBuf};

use clap::Parser;

use miette::{bail, IntoDiagnostic, Result};
use minisign::KeyPair;

use super::{key_args::PasswordArgs, Context, CryptoArgs};

/// Generate a new keypair.
#[derive(Debug, Clone, Parser)]
pub struct KeygenArgs {
	/// File to write the secret key to.
	///
	/// Defaults to `minisign.key` in the current directory.
	#[arg(long, short, value_name = "FILE", default_value = "minisign.key")]
	pub secret_key: PathBuf,

	/// File to write the public key to.
	///
	/// Defaults to the same as the secret key, but with a `.pub` extension.
	#[arg(long, short, value_name = "FILE")]
	pub public_key: Option<PathBuf>,

	#[command(flatten)]
	pub password: PasswordArgs,

	/// A key description.
	///
	/// This is a free-form string that is included in the key files. It is "untrusted": it is not
	/// authenticated or verified in any way, and can be modified by changing the keyfiles directly.
	///
	/// If this contains any of the following placeholders, they will be replaced: `{keyid}` with
	/// the key ID of the signing key, and `{timestamp}` with the current date and time in RFC3339
	/// format.
	#[arg(long, short, value_name = "TEXT")]
	pub description: Option<String>,

	/// Overwrite the key file(s) if they already exist.
	#[arg(long, short)]
	pub force: bool,
}

pub async fn run(ctx: Context<CryptoArgs, KeygenArgs>) -> Result<()> {
	let KeygenArgs {
		secret_key,
		public_key,
		password,
		description,
		force,
	} = ctx.args_sub;

	if secret_key.exists() && !force {
		bail!("secret key file already exists; use --force to overwrite");
	}

	let public_key = public_key.unwrap_or_else(|| secret_key.with_extension("pub"));
	if public_key.exists() && !force {
		bail!("public key file already exists; use --force to overwrite");
	}

	let password = password.read()?;
	let secret_file = File::create(&secret_key).into_diagnostic()?;
	let public_file = File::create(&public_key).into_diagnostic()?;

	eprintln!("writing new keypair at {secret_key:?} and {public_key:?}");
	KeyPair::generate_and_write_encrypted_keypair(
		secret_file,
		public_file,
		description.as_deref(),
		password,
	)
	.into_diagnostic()
	.map(drop)
}
