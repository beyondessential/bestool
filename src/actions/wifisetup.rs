use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

pub mod serve;
pub mod scan;
pub mod connect;
pub mod forget;

/// Configure wifi (using NetworkManager).
#[derive(Debug, Clone, Parser)]
pub struct WifisetupArgs {
	/// Wifisetup subcommand
	#[command(subcommand)]
	pub action: WifisetupAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum WifisetupAction {
	Serve(serve::ServeArgs),
	Scan(scan::ScanArgs),
	Connect(connect::ConnectArgs),
	Forget(forget::ForgetArgs),
}

pub async fn run(ctx: Context<WifisetupArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		WifisetupAction::Serve(subargs) => serve::run(ctx.with_sub(subargs)).await,
		WifisetupAction::Scan(subargs) => scan::run(ctx.with_sub(subargs)).await,
		WifisetupAction::Connect(subargs) => connect::run(ctx.with_sub(subargs)).await,
		WifisetupAction::Forget(subargs) => forget::run(ctx.with_sub(subargs)).await,
	}
}
