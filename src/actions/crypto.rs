use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod keygen;
pub mod sign;
pub mod verify;

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
	Keygen(keygen::KeygenArgs),
	Sign(sign::SignArgs),
	Verify(verify::VerifyArgs),
}

pub async fn run(ctx: Context<CryptoArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		CryptoAction::Keygen(subargs) => keygen::run(ctx.with_sub(subargs)).await,
		CryptoAction::Sign(subargs) => sign::run(ctx.with_sub(subargs)).await,
		CryptoAction::Verify(subargs) => verify::run(ctx.with_sub(subargs)).await,
	}
}
