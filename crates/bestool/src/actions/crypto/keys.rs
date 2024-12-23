use std::path::PathBuf;

use age::{secrecy::SecretString, x25519, Identity, IdentityFile, Recipient};
use clap::Parser;
use dialoguer::Password;
use miette::{bail, miette, Context as _, IntoDiagnostic as _, Result};
use tokio::fs::read_to_string;

#[derive(Debug, Clone, Parser)]
pub struct KeyArgs {
	/// Path to the key or identity file to use for encrypting/decrypting.
	///
	/// The file can either be:
	/// - an identity file, which contains both a public and secret key, in age format;
	/// - a secret key in Bech32 encoding (usually uppercase);
	/// - when encrypting, a public key in Bech32 encoding (usually lowercase).
	///
	/// When encrypting and provided with a secret key, the corresponding public key will be derived
	/// first; there is no way to encrypt with a secret key such that a file is decodable with the
	/// public key.
	///
	/// There is no support (yet) for password-protected secret key or identity files.
	///
	/// ## Examples
	///
	/// An identity file:
	///
	/// ```identity.txt
	/// # created: 2024-12-20T05:36:10.267871872+00:00
	/// # public key: age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
	/// AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
	/// ```
	///
	/// A public key file:
	///
	/// ```key.pub
	/// age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
	/// ```
	///
	/// A secret key file:
	///
	/// ```key.sec
	/// AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
	/// ```
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-k, --key-path PATH`"))]
	#[arg(short, long, verbatim_doc_comment)]
	pub key_path: Option<PathBuf>,

	/// The key to use for encrypting/decrypting as a string.
	///
	/// This supports either public key or secret keys depending on the operation being done.
	/// It does not support the age identity format (with both public and secret keys).
	///
	/// When encrypting and provided with a secret key, the corresponding public key will be derived
	/// first; there is no way to encrypt with a secret key such that a file is decodable with the
	/// public key.
	///
	/// There is no support for password-protected secret keys.
	///
	/// ## Examples
	///
	/// With a public key:
	///
	/// ```console
	/// --key age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
	/// ```
	///
	/// With a secret key:
	///
	/// ```console
	/// --key AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
	/// ```
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-K, --key STRING`"))]
	#[arg(short = 'K', verbatim_doc_comment, conflicts_with = "key_path")]
	pub key: Option<String>,

	#[command(flatten)]
	pub pass: PassphraseArgs,
}

impl KeyArgs {
	/// Retrieve the secret key from the arguments, if one was provided.
	///
	/// Returns `None` if neither of `--key-path` or `--key` was given.
	///
	/// Use [`Self::require_secret_key`] instead of dealing with the `None` yourself if you need to
	/// have a mandatory interface.
	pub async fn get_secret_key(&self) -> Result<Option<Box<dyn Identity>>> {
		self.secret_key(false).await
	}

	/// Retrieve the secret key from the arguments, and error if none is available.
	///
	/// Use [`Self::get_secret_key`] instead of parsing the error if you need to have optional keys.
	pub async fn require_secret_key(&self) -> Result<Box<dyn Identity>> {
		self.secret_key(true)
			.await
			.transpose()
			.expect("BUG: when required:true, Some must not be produced")
	}

	/// Retrieve the public key from the arguments, if one was provided.
	///
	/// Returns `None` if neither of `--key-path` or `--key` was given.
	///
	/// Use [`Self::require_public_key`] instead of dealing with the `None` yourself if you need to
	/// have a mandatory interface.
	pub async fn get_public_key(&self) -> Result<Option<Box<dyn Recipient + Send>>> {
		self.public_key(false).await
	}

	/// Retrieve the public key from the arguments, and error if none is available.
	///
	/// Use [`Self::get_public_key`] instead of parsing the error if you need to have optional keys.
	pub async fn require_public_key(&self) -> Result<Box<dyn Recipient + Send>> {
		self.public_key(true)
			.await
			.transpose()
			.expect("BUG: when required:true, Some must not be produced")
	}

	async fn secret_key(&self, required: bool) -> Result<Option<Box<dyn Identity>>> {
		match self {
			Self {
				key_path: None,
				key: None,
				..
			} => {
				if required {
					bail!("one of `--key-path` or `--key` must be provided");
				} else {
					Ok(None)
				}
			}
			Self {
				key_path: Some(_),
				key: Some(_),
				..
			} => {
				bail!("one of `--key-path` or `--key` must be provided, not both");
			}
			Self { key: Some(key), .. } => key
				.parse::<x25519::Identity>()
				.map(|sec| Some(Box::new(sec) as _))
				.map_err(|err| miette!("{err}").wrap_err("parsing secret key")),
			Self {
				key_path: Some(path),
				pass,
				..
			} if path.extension().unwrap_or_default() == "age" => {
				let key = tokio::fs::read(path).await.into_diagnostic()?;
				let pass = pass.require().await?;
				let id = age::decrypt(&pass, &key)
					.into_diagnostic()
					.wrap_err("revealing identity file")?;

				parse_id_as_identity(
					&String::from_utf8(id)
						.into_diagnostic()
						.wrap_err("parsing identity file as UTF-8")?,
				)
				.map(Some)
			}
			Self {
				key_path: Some(path),
				..
			} => {
				let key = read_to_string(&path)
					.await
					.into_diagnostic()
					.wrap_err("reading identity file")?;
				parse_id_as_identity(&key).map(Some)
			}
		}
	}

	async fn public_key(&self, required: bool) -> Result<Option<Box<dyn Recipient + Send>>> {
		match self {
			Self {
				key_path: None,
				key: None,
				..
			} => {
				if required {
					bail!("one of `--key-path` or `--key` must be provided");
				} else {
					Ok(None)
				}
			}
			Self {
				key_path: Some(_),
				key: Some(_),
				..
			} => {
				bail!("one of `--key-path` or `--key` must be provided, not both");
			}
			Self { key: Some(key), .. } if key.starts_with("age") => key
				.parse::<x25519::Recipient>()
				.map(|key| Some(Box::new(key) as _))
				.map_err(|err| miette!("{err}").wrap_err("parsing public key")),
			Self { key: Some(key), .. } if key.starts_with("AGE-SECRET-KEY") => key
				.parse::<x25519::Identity>()
				.map(|sec| Some(Box::new(sec.to_public()) as _))
				.map_err(|err| miette!("{err}").wrap_err("parsing key")),
			Self { key: Some(_), .. } => {
				bail!("value passed to `--key` is not a public or secret age key");
			}
			Self {
				key_path: Some(path),
				pass,
				..
			} if path.extension().unwrap_or_default() == "age" => {
				let key = tokio::fs::read(path).await.into_diagnostic()?;
				let pass = pass.require().await?;
				let id = age::decrypt(&pass, &key)
					.into_diagnostic()
					.wrap_err("revealing identity file")?;

				parse_id_as_recipient(
					&String::from_utf8(id)
						.into_diagnostic()
						.wrap_err("parsing identity file as UTF-8")?,
				)
				.map(Some)
			}
			Self {
				key_path: Some(path),
				..
			} => {
				let key = read_to_string(path)
					.await
					.into_diagnostic()
					.wrap_err("reading identity file")?;
				parse_id_as_recipient(&key).map(Some)
			}
		}
	}
}

fn parse_id_as_identity(id: &str) -> Result<Box<dyn Identity>> {
	if id.starts_with("AGE-SECRET-KEY") {
		id.parse::<x25519::Identity>()
			.map(|sec| Box::new(sec) as _)
			.map_err(|err| miette!("{err}").wrap_err("parsing secret key"))
	} else {
		IdentityFile::from_buffer(id.as_bytes())
			.into_diagnostic()
			.wrap_err("parsing identity")?
			.into_identities()
			.into_diagnostic()
			.wrap_err("parsing keys from identity")?
			.pop()
			.ok_or_else(|| miette!("no identity available"))
	}
}

fn parse_id_as_recipient(id: &str) -> Result<Box<dyn Recipient + Send>> {
	if id.starts_with("age") {
		id.parse::<x25519::Recipient>()
			.map(|key| Box::new(key) as _)
			.map_err(|err| miette!("{err}").wrap_err("parsing public key"))
	} else if id.starts_with("AGE-SECRET-KEY") {
		id.parse::<x25519::Identity>()
			.map(|sec| Box::new(sec.to_public()) as _)
			.map_err(|err| miette!("{err}").wrap_err("parsing secret key"))
	} else {
		IdentityFile::from_buffer(id.as_bytes())
			.into_diagnostic()
			.wrap_err("parsing identity")?
			.to_recipients()
			.into_diagnostic()
			.wrap_err("parsing recipients from identity")?
			.pop()
			.ok_or_else(|| miette!("no recipient available in identity"))
	}
}

#[derive(Debug, Clone, Parser)]
pub struct PassphraseArgs {
	/// Path to a file containing a passphrase.
	///
	/// The contents of the file will be trimmed of whitespace.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-P, --passphrase-path PATH`"))]
	#[arg(short = 'P', long)]
	pub passphrase_path: Option<PathBuf>,

	/// A passphrase as a string.
	///
	/// This is extremely insecure, only use when there is no other option. When on an interactive
	/// terminal, make sure to wipe this command line from your history, or better yet not record it
	/// in the first place (in Bash you often can do that by prepending a space to your command).
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--insecure-passphrase STRING`"))]
	#[arg(long, conflicts_with = "passphrase_path")]
	pub insecure_passphrase: Option<SecretString>,
}

impl PassphraseArgs {
	/// Retrieve a passphrase from the user.
	pub async fn require(&self) -> Result<Passphrase> {
		self.get(false).await
	}

	/// Retrieve a passphrase from the user, with confirmation when prompting.
	pub async fn require_with_confirmation(&self) -> Result<Passphrase> {
		self.get(true).await
	}

	async fn get(&self, confirm: bool) -> Result<Passphrase> {
		if let Some(ref phrase) = self.insecure_passphrase {
			Ok(Passphrase::new(phrase.clone()))
		} else if let Some(ref path) = self.passphrase_path {
			Ok(Passphrase::new(
				read_to_string(path)
					.await
					.into_diagnostic()
					.wrap_err("reading keyfile")?
					.trim()
					.into(),
			))
		} else {
			let mut prompt = Password::new().with_prompt("Passphrase");
			if confirm {
				prompt = prompt.with_confirmation("Confirm passphrase", "Passphrases mismatching");
			}
			let phrase = prompt.interact().into_diagnostic()?;
			Ok(Passphrase::new(phrase.into()))
		}
	}
}

pub struct Passphrase(age::scrypt::Recipient, age::scrypt::Identity);

impl Passphrase {
	fn new(secret: SecretString) -> Self {
		Self(
			age::scrypt::Recipient::new(secret.clone()),
			age::scrypt::Identity::new(secret),
		)
	}
}

impl Recipient for Passphrase {
	fn wrap_file_key(
		&self,
		file_key: &age_core::format::FileKey,
	) -> std::result::Result<
		(
			Vec<age_core::format::Stanza>,
			std::collections::HashSet<String>,
		),
		age::EncryptError,
	> {
		self.0.wrap_file_key(file_key)
	}
}

impl Identity for Passphrase {
	fn unwrap_stanza(
		&self,
		stanza: &age_core::format::Stanza,
	) -> Option<std::result::Result<age_core::format::FileKey, age::DecryptError>> {
		self.1.unwrap_stanza(stanza)
	}

	fn unwrap_stanzas(
		&self,
		stanzas: &[age_core::format::Stanza],
	) -> Option<std::result::Result<age_core::format::FileKey, age::DecryptError>> {
		self.1.unwrap_stanzas(stanzas)
	}
}
