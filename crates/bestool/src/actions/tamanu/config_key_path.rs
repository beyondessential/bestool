use clap::Parser;
use miette::Result;

use bestool_tamanu::secret_key::locate;

use crate::actions::{
	Context,
	tamanu::{TamanuArgs, find_tamanu},
};

/// Print where this host keeps the Tamanu config key.
///
/// The key encrypts every value in `local_system_secrets`: the settings PSK
/// (and so every secret setting), the device key, and a facility's sync
/// password. A database restored onto a host holding a different key reads none
/// of them.
///
/// A bare-metal or Windows install prints its `crypto.keyFile`. A containerised
/// install holds the key as a podman secret with no server-side path, so it
/// prints the secret's name instead (as `podman secret <name>`). Backing either
/// up is the `[tamanu_secret_key]` backup method's job; this is for reading by
/// hand.
#[derive(Debug, Clone, Parser)]
pub struct ConfigKeyPathArgs {
	/// Package to read the config for (central-server or facility-server).
	///
	/// Detected from the config when not given.
	#[arg(short, long)]
	pub package: Option<String>,
}

pub async fn run(args: ConfigKeyPathArgs, ctx: Context) -> Result<()> {
	let (_, root) = find_tamanu(ctx.require::<TamanuArgs>()).await?;
	println!("{}", locate(&root, args.package.as_deref()).await?);
	Ok(())
}
