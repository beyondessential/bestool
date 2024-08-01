use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod hash;

/// Cryptographic operations.
#[derive(Debug, Clone, Parser)]
pub struct CryptoArgs {
	/// Crypto subcommand
	#[command(subcommand)]
	pub action: CryptoAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CryptoAction {
	Hash(hash::HashArgs),
}

pub async fn run(ctx: Context<CryptoArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		CryptoAction::Hash(subargs) => hash::run(ctx.with_sub(subargs)).await,
	}
}
