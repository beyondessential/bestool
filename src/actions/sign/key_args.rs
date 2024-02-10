use std::{fs::read_to_string, path::PathBuf};

use base64ct::{Base64, Encoding};
use clap::Parser;
use miette::{bail, IntoDiagnostic, Result};
use minisign::{PublicKey, PublicKeyBox, SecretKey, SecretKeyBox};

#[derive(Debug, Clone, Parser)]
pub(crate) struct SecretKeyArgs {
	/// The secret key to sign with.
	///
	/// Prefer to use `--key-file` or `--key-env` instead of this.
	#[arg(long, value_name = "KEY", required_unless_present_any = &["key_file", "key_env"])]
	pub key: Option<String>,

	/// The secret key to sign with, read from a file.
	#[arg(long, value_name = "FILE", required_unless_present_any = &["key", "key_env"])]
	pub key_file: Option<PathBuf>,

	/// The secret key to sign with, read from an environment variable.
	#[arg(long, value_name = "ENVVAR", required_unless_present_any = &["key", "key_file"])]
	pub key_env: Option<String>,

	#[command(flatten)]
	pub password: PasswordArgs,
}

impl SecretKeyArgs {
	pub fn read(&self) -> Result<SecretKey> {
		let password = self.password.read()?;
		match &self {
			Self { key: Some(key), .. } => Self::from_string(key, password),
			Self {
				key_env: Some(env), ..
			} => Self::from_string(&std::env::var(env).into_diagnostic()?, password),
			Self {
				key_file: Some(file),
				..
			} => {
				// we'll always assume it's a full minisign key file
				SecretKey::from_file(file, password).into_diagnostic()
			}
			_ => bail!("exactly one of --key, --key-file, or --key-env must be provided"),
		}
	}

	fn from_string(s: &str, password: Option<String>) -> Result<SecretKey> {
		// try parsing as the raw key as base64 first
		if let Ok(key) = Base64::decode_vec(s) {
			return SecretKey::from_bytes(&key).into_diagnostic();
		}

		// then as the full minisign key file
		SecretKeyBox::from_string(s)
			.into_diagnostic()?
			.into_secret_key(password)
			.into_diagnostic()
	}
}

#[derive(Debug, Clone, Parser)]
pub(crate) struct PublicKeyArgs {
	/// The public key to check signatures with.
	///
	/// Prefer to use `--key-file` or `--key-env` instead of this.
	#[arg(long, value_name = "KEY", required_unless_present_any = &["key_file", "key_env"])]
	pub key: Option<String>,

	/// The public key, read from a file.
	#[arg(long, value_name = "FILE", required_unless_present_any = &["key", "key_env"])]
	pub key_file: Option<PathBuf>,

	/// The public key, read from an environment variable.
	#[arg(long, value_name = "ENVVAR", required_unless_present_any = &["key", "key_file"])]
	pub key_env: Option<String>,
}

impl PublicKeyArgs {
	pub fn read(&self) -> Result<PublicKey> {
		match &self {
			Self { key: Some(key), .. } => Self::from_string(key),
			Self {
				key_env: Some(env), ..
			} => Self::from_string(&std::env::var(env).into_diagnostic()?),
			Self {
				key_file: Some(file),
				..
			} => {
				// we'll always assume it's a full minisign key file
				PublicKey::from_file(file).into_diagnostic()
			}
			_ => bail!("exactly one of --key, --key-file, or --key-env must be provided"),
		}
	}

	fn from_string(s: &str) -> Result<PublicKey> {
		// try parsing as the raw key as base64 first
		if let Ok(key) = Base64::decode_vec(s) {
			return PublicKey::from_bytes(&key).into_diagnostic();
		}

		// then as the full minisign key file
		PublicKeyBox::from_string(s)
			.into_diagnostic()?
			.into_public_key()
			.into_diagnostic()
	}
}

#[derive(Debug, Clone, Parser)]
pub(crate) struct PasswordArgs {
	/// The secret key's password in plain text.
	///
	/// Prefer to use `--password-file` or `--password-env` instead of this.
	#[arg(long, value_name = "KEY", conflicts_with_all = &["password_file", "password_env"])]
	pub password: Option<String>,

	/// The secret key's password, read from a file.
	#[arg(long, value_name = "FILE", conflicts_with_all = &["password", "password_env"])]
	pub password_file: Option<PathBuf>,

	/// The secret key's password, read from an environment variable.
	#[arg(long, value_name = "ENVVAR", conflicts_with_all = &["password", "password_file"])]
	pub password_env: Option<String>,

	/// Prompt for the password interactively.
	///
	/// Do not use this in scripts or CI.
	#[arg(long)]
	pub password_prompt: bool,
}

impl PasswordArgs {
	pub fn read(&self) -> Result<Option<String>> {
		// TODO: zero-box the password to avoid it lingering in memory
		match &self {
			Self {
				password_prompt: true,
				..
			} => Ok(None),
			Self {
				password: Some(pass),
				..
			} => Ok(Some(pass.into())),
			Self {
				password_env: Some(env),
				..
			} => std::env::var(env).into_diagnostic().map(Some),
			Self {
				password_file: Some(file),
				..
			} => read_to_string(file).into_diagnostic().map(Some),
			_ => Ok(Some("".into())), // no password
		}
	}
}
