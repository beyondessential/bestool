use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod upgrade;

/// Manage Caddy.
#[derive(Debug, Clone, Parser)]
pub struct CaddyArgs {
	/// Caddy subcommand
	#[command(subcommand)]
	pub action: CaddyAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CaddyAction {
	Upgrade(upgrade::UpgradeArgs),
}

pub async fn run(args: CaddyArgs, mut ctx: Context) -> Result<()> {
	let action = args.action.clone();
	ctx.provide(args);
	match action {
		CaddyAction::Upgrade(subargs) => upgrade::run(subargs, ctx).await,
	}
}
