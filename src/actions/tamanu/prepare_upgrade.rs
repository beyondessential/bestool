use std::{
	collections::HashSet,
	fs,
	path::{Path, PathBuf},
};

use clap::Parser;
use itertools::Itertools;
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use node_semver::Version;
use regex::Regex;
use tracing::info;

use crate::actions::{
	tamanu::{
		download::{download, make_url},
		find_package,
	},
	Context,
};

use super::{download::ServerKind, find_existing_version, find_tamanu, TamanuArgs};

/// Perform pre-upgrade tasks.
///
/// This will not incur downtime.
///
/// This command will detect which server is installed (Facility or Central) and which version is
/// currently running, then download the desired newer version, unpack it, copy config across,
/// install dependencies, and perform readiness checks.
#[derive(Debug, Clone, Parser)]
pub struct PrepareUpgrade {
	/// Version to update to.
	#[arg(value_name = "VERSION")]
	pub version: Version,

	/// Package to upgrade.
	///
	/// By default, this command detects which server is installed.
	///
	/// If both central and facility servers are present, it will error and you'll have to specify
	/// this option.
	#[arg(short, long)]
	pub package: Option<String>,

	/// Force installing older Tamanu
	#[arg(long)]
	pub force: bool,
}

pub async fn run(ctx: Context<TamanuArgs, PrepareUpgrade>) -> Result<()> {
	let PrepareUpgrade {
		version: new_version,
		package: _,
		force,
	} = ctx.args_sub;

	let (_, root) = find_tamanu(&ctx.args_top)?;
	let existing_version = find_existing_version()?;

	let package = match ctx.args_sub.package {
		Some(package) => package,
		None => find_package(&root)?,
	};
	info!(?package, "using");
	let kind = match package.as_str() {
		"central-server" => ServerKind::Central,
		"facility-server" => ServerKind::Facility,
		_ => bail!("package {package} not recognised"),
	};

	// FIXME: this may not support for Linux as directory structures are different
	// `root` should have a parent.
	let existing_root = root
		.parent()
		.unwrap()
		.join(format!("release-v{existing_version}"));
	let new_root = root
		.parent()
		.unwrap()
		.join(format!("release-v{new_version}"));
	let new_web_root = root
		.parent()
		.unwrap()
		.join(format!("tamanu-web-{new_version}"));

	let minimum_version = Version::parse("2.0.0").unwrap();
	if existing_version < minimum_version || new_version < minimum_version {
		bail!("`PreUpgrade-Tamanu` only support upgrading from/to versions from 2.0.0 onwards");
	}

	if !force && (new_version <= existing_version) {
		bail!("the specified version is older than or equal to the installed version");
	}

	if !new_root.exists() {
		let url = make_url(kind, new_version.to_string())?;
		// FIXME: this may not support for Linux as directory structures are different
		download(url, root.parent().unwrap()).await?;
	}

	if !new_web_root.exists() {
		let url = make_url(ServerKind::Web, new_version.to_string())?;
		// FIXME: this may not support for Linux as directory structures are different
		download(url, root.parent().unwrap()).await?;
	}

	duct::cmd!("pwsh", "-Command", "yarn")
		.dir(&new_root)
		.run()
		.into_diagnostic()
		.wrap_err("failed to run yarn")?;

	let config_path = ["packages", &package, "config", "local.json5"]
		.iter()
		.collect::<PathBuf>();
	let existing_config = existing_root.join(&config_path);
	let new_config = new_root.join(&config_path);
	info!(config = ?existing_config.display(), "copying configs to the new Tamanu installation");
	fs::copy(existing_config, new_config).into_diagnostic()?;

	info!("checking the new version is runnable");
	duct::cmd!("node", "dist", "help")
		.dir(&new_root.join("packages").join(&package))
		.run()
		.into_diagnostic()?;

	if has_non_deterministic_migrations(&existing_root, &new_root)? {
		println!("Warning: the upgrade may contain (a) non-deterministic migration(s).");
	}

	Ok(())
}

/// Checks if there's new migration since the last update.
fn has_non_deterministic_migrations(
	existing_root: impl AsRef<Path>,
	new_root: impl AsRef<Path>,
) -> Result<bool> {
	let re = Regex::new(r"NON_DETERMINISTIC += +true").unwrap();
	let existing_migrations: HashSet<_> = fs::read_dir(
		existing_root
			.as_ref()
			.join(r"packages\shared\src\migrations"),
	)
	.into_diagnostic()?
	.map_ok(|e| e.file_name())
	.try_collect()
	.into_diagnostic()
	.wrap_err_with(|| format!("failed to read migrations in {:?}", existing_root.as_ref()))?;

	fs::read_dir(new_root.as_ref().join(r"packages\shared\src\migrations"))
		.into_diagnostic()?
		.map(|res| {
			res.into_diagnostic()
				.wrap_err_with(|| format!("failed to read migrations in {:?}", new_root.as_ref()))
		})
		.filter_ok(|e| !existing_migrations.contains(&e.file_name()))
		.map(|res| {
			res.and_then(|e| {
				fs::read_to_string(e.path())
					.into_diagnostic()
					.wrap_err_with(|| format!("failed to read file {e:?}"))
			})
		})
		.process_results(|mut iter| iter.any(|m| re.is_match(&m)))
}
