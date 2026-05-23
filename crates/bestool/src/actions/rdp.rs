use clap::{Parser, Subcommand};
use miette::Result;

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
	[RdpArgs => |args: RdpArgs, mut ctx: Context| -> Result<(Action, Context)> {
		let action = args.action.clone();
		ctx.provide(args);
		Ok((action, ctx))
	}]

	monitor => Monitor(MonitorArgs),
	service => Service(ServiceArgs)
}
