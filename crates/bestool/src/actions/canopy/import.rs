use std::path::PathBuf;

use algae_cli::passphrases::PassphraseArgs;
use bestool_canopy::registration;
use clap::Parser;
use miette::{Result, WrapErr as _};

use crate::actions::Context;

/// Import a canopy registration exported from another machine.
///
/// Decrypts the export blob with its passphrase and re-stores it under this
/// machine's identity, so the registration is bound to this host going forward.
#[derive(Debug, Clone, Parser)]
pub struct ImportArgs {
	/// Base64 export blob. Read from stdin if omitted.
	pub blob: Option<String>,

	/// Directory to write the encrypted canopy registration to.
	///
	/// Defaults to the platform's machine-global config directory
	/// (`/etc/bestool`, or `%ProgramData%\bestool` on Windows).
	#[arg(long, value_name = "DIR")]
	pub config: Option<PathBuf>,

	#[command(flatten)]
	#[allow(missing_docs, reason = "don't interfere with clap")]
	pub passphrase: PassphraseArgs,
}

pub async fn run(args: ImportArgs, _ctx: Context) -> Result<()> {
	let ImportArgs {
		blob,
		config,
		passphrase,
	} = args;

	let dir = config.unwrap_or_else(registration::default_dir);
	// Elevate now if we can't write the registration, before prompting for the
	// export passphrase (otherwise we'd fail only after the operator typed it).
	super::ensure_writable_or_reexec(&dir)?;

	let blob_b64 = match blob {
		Some(b) => b,
		None => super::read_stdin("export blob")?,
	};
	let bytes = super::decode_base64(blob_b64.trim())?;

	let pass = passphrase.require().await?;
	let reg = registration::decrypt_with_passphrase(&bytes, pass)
		.wrap_err("decrypting export (wrong passphrase?)")?;

	registration::store_in(&dir, &reg)
		.await
		.wrap_err("storing canopy registration")?;

	println!("Imported canopy registration.");
	if let Some(id) = &reg.server_id {
		println!("  server id: {id}");
	}
	if let Some(id) = &reg.device_id {
		println!("  device id: {id}");
	}
	Ok(())
}
