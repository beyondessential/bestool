use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod cancel;
pub mod confirm;
pub mod file;
pub mod list;
pub mod preauth;
pub mod status;
pub mod token;

/// Upload files to S3.
#[derive(Debug, Clone, Parser)]
pub struct UploadArgs {
	/// Upload subcommand
	#[command(subcommand)]
	pub action: UploadAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum UploadAction {
	Cancel(cancel::CancelArgs),
	Confirm(confirm::ConfirmArgs),
	File(file::FileArgs),
	List(list::ListArgs),
	Preauth(preauth::PreauthArgs),
	Status(status::StatusArgs),
}

pub async fn run(ctx: Context<UploadArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		UploadAction::Cancel(subargs) => cancel::run(ctx.with_sub(subargs)).await,
		UploadAction::Confirm(subargs) => confirm::run(ctx.with_sub(subargs)).await,
		UploadAction::File(subargs) => file::run(ctx.with_sub(subargs)).await,
		UploadAction::List(subargs) => list::run(ctx.with_sub(subargs)).await,
		UploadAction::Preauth(subargs) => preauth::run(ctx.with_sub(subargs)).await,
		UploadAction::Status(subargs) => status::run(ctx.with_sub(subargs)).await,
	}
}
