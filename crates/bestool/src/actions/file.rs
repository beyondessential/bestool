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
	[Context<FileArgs> => {|ctx: Context<FileArgs>| -> Result<(Action, Context<FileArgs>)> {
		Ok((ctx.args_top.action.clone(), ctx.with_sub(())))
	}}](with_sub)

	join => Join(JoinArgs),
	split => Split(SplitArgs)
}
