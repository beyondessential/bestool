use clap::{Parser, Subcommand};
use miette::Result;

use crate::args::Args;

use super::Context;

/// File utilities.
#[derive(Debug, Clone, Parser)]
pub struct FileArgs {
	/// Subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[Context<Args, FileArgs> => {|ctx: Context<Args, FileArgs>| -> Result<(Action, Context<FileArgs>)> {
		Ok((ctx.args_sub.action.clone(), ctx.push(())))
	}}](with_sub)

	join => Join(JoinArgs),
	split => Split(SplitArgs)
}
