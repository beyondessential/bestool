use clap::Parser;
use miette::Result;

use crate::args::Args;

use super::Context;

/// Generate markdown documentation
#[derive(Debug, Clone, Parser)]
pub struct DocsArgs {}

pub async fn run(_args: DocsArgs, _ctx: Context) -> Result<()> {
	let markdown = clap_markdown::help_markdown::<Args>();
	println!("{}", markdown);
	Ok(())
}
