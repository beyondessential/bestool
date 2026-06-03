use std::path::PathBuf;

use algae_cli::passphrases::Passphrase;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bestool_canopy::registration;
use clap::Parser;
use miette::{Result, WrapErr as _, bail};

use crate::actions::Context;

/// Export this machine's canopy registration for transfer to another machine.
///
/// Decrypts the local registration, re-encrypts it under a freshly generated
/// passphrase, and prints the base64 blob and the passphrase. Carry the blob
/// and the passphrase on *separate* channels — together they're enough to
/// enrol the other machine.
#[derive(Debug, Clone, Parser)]
pub struct ExportArgs {
	/// Directory holding the encrypted canopy registration.
	///
	/// Defaults to the platform's machine-global config directory
	/// (`/etc/bestool`, or `%ProgramData%\bestool` on Windows).
	#[arg(long, value_name = "DIR")]
	pub config: Option<PathBuf>,
}

pub async fn run(args: ExportArgs, _ctx: Context) -> Result<()> {
	let dir = args
		.config
		.clone()
		.unwrap_or_else(registration::default_dir);
	let Some(reg) = super::load_registration(args.config.as_deref())
		.await
		.wrap_err("reading canopy registration")?
	else {
		bail!("no canopy registration to export at {}", dir.display());
	};

	let passphrase = registration::generate_passphrase()?;
	let blob = registration::encrypt_with_passphrase(&reg, Passphrase::new(passphrase.clone().into()))
		.wrap_err("encrypting registration for export")?;
	let encoded = STANDARD.encode(&blob);

	println!("Canopy registration export.");
	println!("Send the blob and the passphrase on SEPARATE channels.");
	println!();
	println!("passphrase: {passphrase}");
	println!();
	println!("{encoded}");
	Ok(())
}
