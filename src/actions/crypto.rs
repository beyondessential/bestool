use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod check;
pub mod sign;
pub mod keygen;

mod inout_args;
mod key_args;

/// Cryptographic operations.
#[derive(Debug, Clone, Parser)]
pub struct CryptoArgs {
	/// Crypto subcommand
	#[command(subcommand)]
	pub action: CryptoAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CryptoAction {
	Verify(check::CheckArgs),
	Sign(sign::SignArgs),
	Keygen(keygen::KeygenArgs),
}

pub async fn run(ctx: Context<SignArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		CryptoAction::Check(subargs) => check::run(ctx.with_sub(subargs)).await,
		CryptoAction::Sign(subargs) => sign::run(ctx.with_sub(subargs)).await,
		CryptoAction::Keygen(subargs) => keygen::run(ctx.with_sub(subargs)).await,
	}
}
