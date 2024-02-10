use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod files;
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
	Files(files::FilesArgs),
}

pub async fn run(ctx: Context<SignArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		SignAction::Files(subargs) => files::run(ctx.with_sub(subargs)).await,
	}
}
