use std::{io::SeekFrom, path::Path};

use clap::{Parser, Subcommand};
use fs4::tokio::AsyncFileExt;
use miette::{miette, IntoDiagnostic, Result, WrapErr};
use ssh_key::{authorized_keys::AuthorizedKeys, PublicKey};
use tokio::{
	fs::{create_dir_all, try_exists, File, OpenOptions},
	io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
use tracing::{debug, info, warn};

use super::Context;

/// SSH helpers.
#[derive(Debug, Clone, Parser)]
pub struct SshArgs {
	/// SSH subcommand
	#[command(subcommand)]
	pub action: SshAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SshAction {
	AddKey(AddKeyArgs),
}

pub async fn run(ctx: Context<SshArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		SshAction::AddKey(subargs) => add_key(ctx.with_sub(subargs)).await,
	}
}

/// Add a public key to the current user's authorized_keys file.
///
/// On Unix, this is equivalent to `echo 'public key' >> ~/.ssh/authorized_keys`, except that this
/// command will check public keys are well-formed and will never accidentally overwrite the file.
///
/// On Windows, this behaves differently whether the current user is a regular user or an
/// administrator, as the file that needs to be written is different. Additionally, it will ensure
/// that file ACLs are correct when used for administrators.
///
/// This tool will obtain an exclusive lock on the file to prevent concurrent modification, which
/// could result in a loss of data. It will also check the validity of the file before writing it.
#[derive(Debug, Clone, Parser)]
pub struct AddKeyArgs {
	/// SSH public key to add.
	///
	/// Multiple keys may be provided, which will behave the same as calling this command
	/// multiple times with each different key.
	///
	/// Keys that already exist are automatically excluded so they're not written twice.
	#[arg(required = true)]
	pub keys: Vec<String>,
}

pub async fn add_key(ctx: Context<SshArgs, AddKeyArgs>) -> Result<()> {
	let AddKeyArgs { keys } = ctx.args_sub;

	info!("checking public keys are well-formed");
	let mut valid_keys = keys
		.iter()
		.map(|key| {
			PublicKey::from_openssh(key)
				.into_diagnostic()
				.wrap_err_with(|| format!("parsing public key: {key}"))
		})
		.collect::<Result<Vec<PublicKey>>>()?;

	let filepath = match (is_root::is_root(), cfg!(windows)) {
		(true, true) => Path::new(r"C:\ProgramData\ssh\administrators_authorized_keys").into(),
		(true, false) => Path::new("/root/.ssh/authorized_keys").into(),
		(false, _) => dirs::home_dir()
			.ok_or_else(|| miette!("can't find home directory"))?
			.join(".ssh")
			.join("authorized_keys"),
	};
	info!(?filepath, "determined location of authorized_keys file");

	if !try_exists(&filepath).await.into_diagnostic()? {
		if let Some(dir) = filepath.parent() {
			create_dir_all(dir).await.into_diagnostic()?;
		}
		File::create(&filepath).await.into_diagnostic()?;
		info!(?filepath, "created empty authorized_keys file");
	}

	#[cfg(windows)]
	if is_root::is_root() {
		duct::cmd!(
			"icacls.exe",
			&filepath,
			"/inheritance:r",
			"/grant",
			"Administrators:F",
			"/grant",
			"SYSTEM:F"
		)
		.run()
		.into_diagnostic()?;
		info!("set proper permissions on file");
	}

	debug!("open and lock file");
	let mut file = OpenOptions::new()
		.read(true)
		.write(true)
		.open(&filepath)
		.await
		.into_diagnostic()?;
	file.lock_exclusive()
		.into_diagnostic()
		.wrap_err("failed to obtain exclusive lock")?;

	let mut data = String::new();
	let bytes = file.read_to_string(&mut data).await.into_diagnostic()?;
	debug!(bytes, "read file");

	let parser = AuthorizedKeys::new(&data);
	for entry in parser {
		let entry = entry
			.into_diagnostic()
			.wrap_err("authorized_keys file is invalid")?;
		debug!("excluding already-present keys from input");
		valid_keys.retain(|key| key != entry.public_key());
	}

	if valid_keys.is_empty() {
		warn!("all input keys are already in authorized_keys");
		return Ok(());
	}

	if !data.ends_with('\n') && !data.is_empty() {
		data.push('\n');
	}

	for key in valid_keys {
		data.push_str(&key.to_openssh().into_diagnostic()?);
		data.push('\n');
	}

	let parser = AuthorizedKeys::new(&data);
	for entry in parser {
		entry.into_diagnostic().wrap_err(
			"something went really wrong: new authorized_keys file is invalid, not writing it",
		)?;
	}

	file.seek(SeekFrom::Start(0)).await.into_diagnostic()?;
	file.set_len(0).await.into_diagnostic()?;
	file.write_all(data.as_bytes()).await.into_diagnostic()?;
	info!(bytes = data.len(), "wrote new file");

	debug!("unlock file");
	file.unlock().into_diagnostic()?;

	Ok(())
}
