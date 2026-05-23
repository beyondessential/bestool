use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

/// File utilities.
#[derive(Debug, Clone, Parser)]
pub struct FileArgs {
	/// Subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[FileArgs => |args: FileArgs, mut ctx: Context| -> Result<(Action, Context)> {
		let action = args.action.clone();
		ctx.provide(args);
		Ok((action, ctx))
	}]

	join => Join(JoinArgs),
	split => Split(SplitArgs)
}
