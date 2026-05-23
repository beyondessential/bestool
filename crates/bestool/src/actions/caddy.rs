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

pub async fn run(args: CaddyArgs, mut ctx: Context) -> Result<()> {
	let action = args.action.clone();
	ctx.provide(args);
	match action {
		CaddyAction::ConfigureTamanu(subargs) => configure_tamanu::run(subargs, ctx).await,
		CaddyAction::Download(subargs) => download::run(subargs, ctx).await,
	}
}
