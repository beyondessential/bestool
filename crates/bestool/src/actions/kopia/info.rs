use std::process::Stdio;

use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};

use super::{KopiaArgs, common::kopia_binary};
use crate::actions::Context;

/// Show kopia repository connection status.
///
/// Wraps `kopia repository status`. Useful as a quick check that the
/// configured repository is reachable and we're connected.
#[derive(Debug, Clone, Parser)]
pub struct InfoArgs {}

pub async fn run(_args: InfoArgs, ctx: Context) -> Result<()> {
	let kopia = ctx.require::<KopiaArgs>();
	let bin = kopia_binary(kopia)?;

	let status = std::process::Command::new(&bin)
		.args(["repository", "status"])
		.env("KOPIA_CHECK_FOR_UPDATES", "false")
		.stdin(Stdio::inherit())
		.stdout(Stdio::inherit())
		.stderr(Stdio::inherit())
		.status()
		.into_diagnostic()
		.wrap_err_with(|| format!("invoking {}", bin.display()))?;

	if !status.success() {
		std::process::exit(status.code().unwrap_or(1));
	}

	Ok(())
}
