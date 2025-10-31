use clap::{Parser, Subcommand};
use miette::Result;

use crate::args::Args;

use super::Context;

/// Cryptographic operations.
#[derive(Debug, Clone, Parser)]
pub struct CryptoArgs {
	/// Crypto subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[Context<Args, CryptoArgs> => {|ctx: Context<Args, CryptoArgs>| -> Result<(Action, Context<CryptoArgs>)> {
		Ok((ctx.args_sub.action.clone(), ctx.push(())))
	}}](with_sub)

	decrypt => Decrypt(DecryptArgs),
	encrypt => Encrypt(EncryptArgs),
	hash => Hash(HashArgs),
	keygen => Keygen(KeygenArgs),
	protect => Protect(ProtectArgs),
	reveal => Reveal(RevealArgs)
}
