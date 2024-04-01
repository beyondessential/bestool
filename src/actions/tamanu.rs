use std::path::PathBuf;

use clap::{Parser, Subcommand};
use miette::{miette, IntoDiagnostic, Result};
use node_semver::Version;

use super::Context;

pub mod config;
pub mod download;
pub mod find;
pub mod roots;
pub mod psql;

/// Interact with Tamanu.
#[derive(Debug, Clone, Parser)]
pub struct TamanuArgs {
	/// Tamanu root to operate in
	#[arg(long)]
	pub root: Option<PathBuf>,

	/// Tamanu subcommand
	#[command(subcommand)]
	pub action: TamanuAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum TamanuAction {
	Config(config::ConfigArgs),
	Download(download::DownloadArgs),
	Find(find::FindArgs),
	Psql(psql::PsqlArgs),
}

pub async fn run(ctx: Context<TamanuArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		TamanuAction::Config(subargs) => config::run(ctx.with_sub(subargs)).await,
		TamanuAction::Download(subargs) => download::run(ctx.with_sub(subargs)).await,
		TamanuAction::Find(subargs) => find::run(ctx.with_sub(subargs)).await,
		TamanuAction::Psql(subargs) => psql::run(ctx.with_sub(subargs)).await,
	}
}

pub fn find_tamanu(args: &TamanuArgs) -> Result<(Version, PathBuf)> {
	if let Some(root) = &args.root {
		let version = roots::version_of_root(root)?
			.ok_or_else(|| miette!("no tamanu found in --root={root:?}"))?;
		Ok((version, root.canonicalize().into_diagnostic()?))
	} else {
		roots::find_versions()?
			.into_iter()
			.next()
			.ok_or_else(|| miette!("no tamanu discovered, use --root"))
	}
}
