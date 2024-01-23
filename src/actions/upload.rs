use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod delegate;
pub mod files;
pub mod list;

/// Upload files to S3.
#[derive(Debug, Clone, Parser)]
pub struct UploadArgs {
	/// Upload subcommand
	#[command(subcommand)]
	pub action: UploadAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum UploadAction {
	Files(files::FilesArgs),
	List(list::ListArgs),
	Delegate(delegate::DelegateArgs),
}

pub async fn run(ctx: Context<UploadArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		UploadAction::Files(subargs) => files::run(ctx.with_sub(subargs)).await,
		UploadAction::List(subargs) => list::run(ctx.with_sub(subargs)).await,
		UploadAction::Delegate(subargs) => delegate::run(ctx.with_sub(subargs)).await,
	}
}

#[derive(Debug, Clone)]
pub struct UploadId {
	pub bucket: String,
	pub key: String,
	pub id: String,
}
