use clap::Parser;
use miette::Result;

use crate::args::Args;

use super::Context;

/// Generate markdown documentation for all subcommands (hidden command for maintainers).
#[derive(Debug, Clone, Parser)]
pub struct DocsArgs {}

pub async fn run(_ctx: Context<Args, DocsArgs>) -> Result<()> {
	let markdown = clap_markdown::help_markdown::<Args>();
	println!("{}", markdown);
	Ok(())
}
