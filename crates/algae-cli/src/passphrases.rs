use std::{io::IsTerminal, path::PathBuf};

use age::{Identity, Recipient};
use clap::Parser;
use dialoguer::Password;
use miette::{bail, miette, Context as _, IntoDiagnostic as _, Result};
use pinentry::PassphraseInput;
use tokio::fs::read_to_string;

/// Re-exported from [`age`] so dependents can name and read passphrase secrets
/// without taking a direct dependency on age.
pub use age::secrecy::{ExposeSecret, SecretString};

/// [Clap][clap] arguments for passphrases.
///
/// ```no_run
/// use clap::Parser;
/// use miette::Result;
/// use algae_cli::passphrases::PassphraseArgs;
///
/// /// Your CLI tool
/// #[derive(Parser)]
/// struct Args {
///     #[command(flatten)]
///     pass: PassphraseArgs,
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let args = Args::parse();
///     let key = args.pass.require().await?;
///     // use key somehow...
/// # let _key = key;
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone, Parser)]
pub struct PassphraseArgs {
	/// Path to a file containing a passphrase.
	///
	/// The contents of the file will be trimmed of whitespace.
	#[arg(short = 'P', long)]
	pub passphrase_path: Option<PathBuf>,

	/// A passphrase as a string.
	///
	/// This is extremely insecure, only use when there is no other option. When on an interactive
	/// terminal, make sure to wipe this command line from your history, or better yet not record it
	/// in the first place (in Bash you often can do that by prepending a space to your command).
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
			prompt_passphrase(confirm)
		}
	}
}

/// How pinentry should be driven, given the surrounding environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinentryMode {
	/// No terminal and no display: prompting can never succeed, so don't try.
	Unavailable,
	/// Force pinentry onto the controlling terminal. Used over SSH, where a GUI
	/// pinentry can't render its dialog and would hang forever at `GETPIN`.
	ForceTty,
	/// Let pinentry pick its own backend: a local GUI if a display is present,
	/// otherwise the terminal.
	Default,
}

/// Decide how to drive pinentry from what the environment offers.
///
/// Over SSH a GUI pinentry (gnome3/gtk/qt) has no display it can actually draw
/// on, so it blocks indefinitely; force it onto the terminal instead. With
/// neither a terminal nor a display there's nothing to prompt on at all.
fn pinentry_mode(stdin_is_tty: bool, has_display: bool, over_ssh: bool) -> PinentryMode {
	if over_ssh && stdin_is_tty {
		PinentryMode::ForceTty
	} else if stdin_is_tty || has_display {
		PinentryMode::Default
	} else {
		PinentryMode::Unavailable
	}
}

/// Prompt the user for a passphrase, via pinentry when available.
fn prompt_passphrase(confirm: bool) -> Result<SecretString> {
	let ssh_tty = std::env::var("SSH_TTY").ok().filter(|tty| !tty.is_empty());
	let over_ssh = ssh_tty.is_some() || std::env::var_os("SSH_CONNECTION").is_some();
	let has_display = ["DISPLAY", "WAYLAND_DISPLAY"]
		.into_iter()
		.any(|key| std::env::var_os(key).is_some_and(|val| !val.is_empty()));
	let stdin_is_tty = std::io::stdin().is_terminal();

	let mode = pinentry_mode(stdin_is_tty, has_display, over_ssh);
	if mode == PinentryMode::Unavailable {
		bail!(
			"no terminal or display is available to prompt for a passphrase; pass --passphrase-path or --insecure-passphrase"
		);
	}

	if let Some(mut input) = PassphraseInput::with_default_binary() {
		input
			.with_prompt("Passphrase:")
			.required("Cannot use an empty passphrase");
		if confirm {
			input.with_confirmation("Confirm passphrase:", "Passphrases do not match");
		}

		// Over SSH the inherited DISPLAY/WAYLAND_DISPLAY makes pinentry pick a GUI
		// backend that can't render and hangs at GETPIN. Clear them so it falls back
		// to the curses/tty backend, and point ttyname at the SSH terminal.
		#[cfg(unix)]
		if mode == PinentryMode::ForceTty {
			use pinentry::unix::Options;
			let mut builder = Options::builder().x11_display("").wayland_display("");
			if let Some(ref tty) = ssh_tty {
				builder = builder.tty_name(tty);
			}
			input.with_unix_options(builder.build());
		}

		input.interact().map_err(|err| miette!("{err}"))
	} else {
		let mut prompt = Password::new().with_prompt("Passphrase");
		if confirm {
			prompt = prompt.with_confirmation("Confirm passphrase", "Passphrases do not match");
		}
		let phrase = prompt.interact().into_diagnostic()?;
		Ok(phrase.into())
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

	/// Initialise from a string, with a fixed scrypt work factor (`N = 2^log_n`).
	///
	/// [`Passphrase::new`] calibrates the work factor to take about one second,
	/// which on a fast machine means hundreds of MiB of scrypt arena (memory
	/// scales with `N`: 128 × 8 × 2^log_n bytes). That hardness only matters
	/// when the secret is a guessable human passphrase; for a high-entropy
	/// generated secret it buys nothing, and a low work factor avoids the
	/// memory and CPU spike.
	///
	/// # Panics
	///
	/// Panics if `log_n == 0` or `log_n >= 64`.
	pub fn with_work_factor(secret: SecretString, log_n: u8) -> Self {
		let mut recipient = age::scrypt::Recipient::new(secret.clone());
		recipient.set_work_factor(log_n);
		Self(recipient, age::scrypt::Identity::new(secret))
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

#[cfg(test)]
mod tests {
	use super::{pinentry_mode, PinentryMode};

	#[test]
	fn ssh_with_terminal_forces_tty() {
		assert_eq!(
			pinentry_mode(true, false, true),
			PinentryMode::ForceTty,
			"interactive SSH session must use the terminal, not a GUI"
		);
		assert_eq!(
			pinentry_mode(true, true, true),
			PinentryMode::ForceTty,
			"a forwarded display over SSH must not override the terminal"
		);
	}

	#[test]
	fn local_terminal_or_display_uses_defaults() {
		assert_eq!(pinentry_mode(true, false, false), PinentryMode::Default);
		assert_eq!(pinentry_mode(false, true, false), PinentryMode::Default);
		assert_eq!(pinentry_mode(true, true, false), PinentryMode::Default);
	}

	#[test]
	fn no_terminal_and_no_display_is_unavailable() {
		assert_eq!(
			pinentry_mode(false, false, false),
			PinentryMode::Unavailable
		);
		assert_eq!(
			pinentry_mode(false, false, true),
			PinentryMode::Unavailable,
			"SSH without a pty (ssh -T) and no display can't prompt"
		);
	}

	#[test]
	fn ssh_without_terminal_but_forwarded_display_uses_defaults() {
		assert_eq!(pinentry_mode(false, true, true), PinentryMode::Default);
	}
}
