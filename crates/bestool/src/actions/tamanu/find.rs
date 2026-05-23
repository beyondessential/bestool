use bestool_tamanu::roots;
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

pub async fn run(args: FindArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();
	let mut versions = if let Some(root) = tamanu.root.as_ref() {
		if let Some(version) = roots::version_of_root(root)? {
			vec![(version, root.canonicalize().into_diagnostic()?)]
		} else {
			bail!("no version found in explicit root {root:?}");
		}
	} else {
		roots::find_versions()?
	};

	if args.asc {
		versions.reverse();
	}

	if let Some(count) = args.count {
		versions.truncate(count);
	}

	for (version, root) in versions {
		if args.with_version {
			println!("[{version}] {}", root.display());
		} else {
			println!("{}", root.display());
		}
	}

	Ok(())
}
