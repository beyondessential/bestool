use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

/// Interact with Canopy (the Tamanu meta-monitoring service).
#[derive(Debug, Clone, Parser)]
pub struct CanopyArgs {
	/// Canopy subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[CanopyArgs => |args: CanopyArgs, mut ctx: Context| -> Result<(Action, Context)> {
		let action = args.action.clone();
		ctx.provide(args);
		Ok((action, ctx))
	}]

	#[cfg(feature = "canopy-register")]
	register => Register(RegisterArgs)
}
