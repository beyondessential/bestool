use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod download;

/// Manage WAL-G.
#[derive(Debug, Clone, Parser)]
pub struct WalgArgs {
	/// WAL-G subcommand
	#[command(subcommand)]
	pub action: WalgAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum WalgAction {
	Download(download::DownloadArgs),
}

pub async fn run(ctx: Context<WalgArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		WalgAction::Download(subargs) => download::run(ctx.with_sub(subargs)).await,
	}
}
