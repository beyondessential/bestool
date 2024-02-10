use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod check;
pub mod files;
pub mod keygen;

mod inout_args;
mod key_args;

/// Sign and verify files.
#[derive(Debug, Clone, Parser)]
pub struct SignArgs {
	/// Sign subcommand
	#[command(subcommand)]
	pub action: SignAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SignAction {
	Check(check::CheckArgs),
	Files(files::FilesArgs),
	Keygen(keygen::KeygenArgs),
}

pub async fn run(ctx: Context<SignArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		SignAction::Check(subargs) => check::run(ctx.with_sub(subargs)).await,
		SignAction::Files(subargs) => files::run(ctx.with_sub(subargs)).await,
		SignAction::Keygen(subargs) => keygen::run(ctx.with_sub(subargs)).await,
	}
}
