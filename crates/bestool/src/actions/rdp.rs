use clap::{Parser, Subcommand};
use miette::Result;

use crate::args::Args;

use super::Context;

pub mod audit;
pub mod events;
pub mod state;
pub mod tailscale;

#[cfg(windows)]
pub mod notify;

/// Windows RDP session tooling.
#[derive(Debug, Clone, Parser)]
pub struct RdpArgs {
	/// RDP subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[Context<Args, RdpArgs> => {|ctx: Context<Args, RdpArgs>| -> Result<(Action, Context<RdpArgs>)> {
		Ok((ctx.args_sub.action.clone(), ctx.push(())))
	}}](with_sub)

	monitor => Monitor(MonitorArgs),
	service => Service(ServiceArgs)
}
