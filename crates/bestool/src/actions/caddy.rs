use clap::{Parser, Subcommand};
use miette::Result;

use crate::args::Args;

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

pub async fn run(ctx: Context<Args, CaddyArgs>) -> Result<()> {
	match ctx.args_sub.action.clone() {
		CaddyAction::ConfigureTamanu(subargs) => configure_tamanu::run(ctx.push(subargs)).await,
		CaddyAction::Download(subargs) => download::run(ctx.push(subargs)).await,
	}
}
