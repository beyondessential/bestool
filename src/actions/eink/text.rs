use clap::{Parser, ValueEnum};
use miette::{miette, IntoDiagnostic, Result, WrapErr};
use crate::actions::Context;

use super::EinkArgs;

/// Print some text.
///
/// By default, this tries to load a few well-known system fonts, then falls back to the first font
/// it can find. To change this, either use a specific with `--font-name` or `--font-file`, or use
/// the `--family`, `--monospace`, `--bold`, and `--italic` selectors to filter the system fonts.
/// If it can't find a matching font, it will fall back; and if it can't find any font it will fail.
#[derive(Debug, Clone, Parser)]
pub struct TextArgs {
}

pub async fn run(ctx: Context<EinkArgs, TextArgs>) -> Result<()> {

	Ok(())
}

