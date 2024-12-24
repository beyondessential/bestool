use std::path::PathBuf;

use age::{secrecy::SecretString, Identity, Recipient};
use clap::Parser;
use dialoguer::Password;
use miette::{miette, Context as _, IntoDiagnostic as _, Result};
use pinentry::PassphraseInput;
use tokio::fs::read_to_string;

/// [Clap][clap] arguments for passphrases.
///
/// ```no_run
/// use clap::Parser;
/// use miette::Result;
/// use algae_cli::passphrase::PassphraseArgs;
///
/// /// Your CLI tool
/// #[derive(Parser)]
/// struct Args {
///     #[command(flatten)]
///     pass: PassphraseArgs,
/// }
///
/// #[tokio::main]
/// fn main() -> Result<()> {
///     let args = Args::parse();
///     let key = args.pass.require().await?;
///     dbg!(key);
///     Ok(())
/// }
/// ```
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

	/// Retrieve a passphrase from the user, as a [`SecretString`].
	pub async fn require_phrase(&self) -> Result<SecretString> {
		self.get_phrase(false).await
	}

	/// Retrieve a passphrase from the user, as a [`SecretString`], with confirmation when prompting.
	pub async fn require_phrase_with_confirmation(&self) -> Result<SecretString> {
		self.get_phrase(true).await
	}

	async fn get(&self, confirm: bool) -> Result<Passphrase> {
		self.get_phrase(confirm).await.map(Passphrase::new)
	}

	async fn get_phrase(&self, confirm: bool) -> Result<SecretString> {
		if let Some(ref phrase) = self.insecure_passphrase {
			Ok(phrase.clone())
		} else if let Some(ref path) = self.passphrase_path {
			Ok(read_to_string(path)
				.await
				.into_diagnostic()
				.wrap_err("reading keyfile")?
				.trim()
				.into())
		} else {
			if let Some(mut input) = PassphraseInput::with_default_binary() {
				input
					.with_prompt("Passphrase:")
					.required("Cannot use an empty passphrase");
				if confirm {
					input.with_confirmation("Confirm passphrase:", "Passphrases do not match");
				}
				input.interact().map_err(|err| miette!("{err}"))
			} else {
				let mut prompt = Password::new().with_prompt("Passphrase");
				if confirm {
					prompt =
						prompt.with_confirmation("Confirm passphrase", "Passphrases do not match");
				}
				let phrase = prompt.interact().into_diagnostic()?;
				Ok(phrase.into())
			}
		}
	}
}

/// A wrapper around [`age::scrypt::Recipient`] and [`age::scrypt::Identity`].
///
/// Such that a single struct implements both [`Recipient`] and [`Identity`]
/// traits for a single passphrase, simplifying usage.
pub struct Passphrase(age::scrypt::Recipient, age::scrypt::Identity);

impl Passphrase {
	/// Initialise from a string.
	pub fn new(secret: SecretString) -> Self {
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
