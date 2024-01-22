use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod file;
pub mod preauth;

/// Upload files to S3.
#[derive(Debug, Clone, Parser)]
pub struct UploadArgs {
	/// Upload subcommand
	#[command(subcommand)]
	pub action: UploadAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum UploadAction {
	File(file::FileArgs),
	Preauth(preauth::PreauthArgs),
}

pub async fn run(ctx: Context<UploadArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		UploadAction::File(subargs) => file::run(ctx.with_sub(subargs)).await,
		UploadAction::Preauth(subargs) => preauth::run(ctx.with_sub(subargs)).await,
	}
}
