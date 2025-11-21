use clap::Parser;
use miette::{IntoDiagnostic, Result, bail};

use crate::actions::Context;

use super::TamanuArgs;

/// Find Tamanu installations.
#[derive(Debug, Clone, Parser)]
pub struct FindArgs {
	/// Return this many entries
	#[arg(long, short = 'n')]
	pub count: Option<usize>,

	/// Sort ascending.
	#[arg(long)]
	pub asc: bool,

	/// With version.
	///
	/// Print parsed version information for each root.
	#[arg(long)]
	pub with_version: bool,
}

pub async fn run(ctx: Context<TamanuArgs, FindArgs>) -> Result<()> {
	let mut versions = if let Some(root) = ctx.args_top.root {
		if let Some(version) = super::roots::version_of_root(&root)? {
			vec![(version, root.canonicalize().into_diagnostic()?)]
		} else {
			bail!("no version found in explicit root {root:?}");
		}
	} else {
		super::roots::find_versions()?
	};

	if ctx.args_sub.asc {
		versions.reverse();
	}

	if let Some(count) = ctx.args_sub.count {
		versions.truncate(count);
	}

	for (version, root) in versions {
		if ctx.args_sub.with_version {
			println!("[{version}] {}", root.display());
		} else {
			println!("{}", root.display());
		}
	}

	Ok(())
}
