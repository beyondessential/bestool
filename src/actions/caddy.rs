use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod configure_tamanu;
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
	ConfigureTamanu(configure_tamanu::ConfigureTamanuArgs),
	Download(download::DownloadArgs),
}

pub async fn run(ctx: Context<CaddyArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		CaddyAction::ConfigureTamanu(subargs) => configure_tamanu::run(ctx.with_sub(subargs)).await,
		CaddyAction::Download(subargs) => download::run(ctx.with_sub(subargs)).await,
	}
}
