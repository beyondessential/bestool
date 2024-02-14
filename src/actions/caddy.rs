use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod download;

/// Manage Caddy.
#[derive(Debug, Clone, Parser)]
pub struct CaddyArgs {
	/// Caddy subcommand
	#[command(subcommand)]
	pub action: CaddyAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CaddyAction {
	Download(download::DownloadArgs),
}

pub async fn run(ctx: Context<CaddyArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		CaddyAction::Download(subargs) => download::run(ctx.with_sub(subargs)).await,
	}
}
