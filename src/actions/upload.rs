use clap::{Parser, Subcommand};
use miette::Result;

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

pub async fn run(args: UploadArgs) -> Result<()> {
	match args.action.clone() {
		UploadAction::File(subargs) => file::run(args, subargs).await,
		UploadAction::Preauth(subargs) => preauth::run(args, subargs).await,
	}
}
