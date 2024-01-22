use clap::Parser;
use miette::{bail, IntoDiagnostic, Result};

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

pub async fn run(args: TamanuArgs, subargs: FindArgs) -> Result<()> {
	let mut versions = if let Some(root) = args.root {
		if let Some(version) = crate::roots::version_of_root(&root)? {
			vec![(version, root.canonicalize().into_diagnostic()?)]
		} else {
			bail!("no version found in explicit root {root:?}");
		}
	} else {
		crate::roots::find_versions()?
	};

	if subargs.asc {
		versions.reverse();
	}

	if let Some(count) = subargs.count {
		versions.truncate(count);
	}

	for (version, root) in versions {
		if subargs.with_version {
			println!("[{version}] {}", root.display());
		} else {
			println!("{}", root.display());
		}
	}

	Ok(())
}
