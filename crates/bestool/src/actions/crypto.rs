use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

/// Cryptographic operations.
#[derive(Debug, Clone, Parser)]
pub struct CryptoArgs {
	/// Crypto subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[Context<CryptoArgs> => {|ctx: Context<CryptoArgs>| -> Result<(Action, Context<CryptoArgs>)> {
		Ok((ctx.args_top.action.clone(), ctx.with_sub(())))
	}}](with_sub)

	decrypt => Decrypt(DecryptArgs),
	encrypt => Encrypt(EncryptArgs),
	hash => Hash(HashArgs),
	keygen => Keygen(KeygenArgs)
}
