use std::{
	collections::HashSet,
	fs,
	path::{Path, PathBuf},
};

use binstalk_downloader::download::{Download, PkgFmt};
use clap::Parser;
use itertools::Itertools;
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use node_semver::Version;
use regex::Regex;
use tracing::{info, warn};

use crate::{actions::Context, download::client};

use super::{
	download::{make_url, ServerKind},
	find_existing_version, find_package, find_tamanu, ApiServerKind, TamanuArgs,
};

pub const UPGRADED_SIGNAL_NAME: &str = ".bestool_preupgraded";

/// Perform pre-upgrade tasks.
///
/// This will not incur downtime.
///
/// This command will detect which server is installed (Facility or Central) and which version is
/// currently running, then download the desired newer version, unpack it, copy config across,
/// install dependencies, and perform readiness checks.
#[cfg_attr(docsrs, doc("\n\n**Command**: `bestool tamanu prepare-upgrade`"))]
#[derive(Debug, Clone, Parser)]
pub struct PrepareUpgradeArgs {
	/// Version to update to.
	#[cfg_attr(docsrs, doc("\n\n**Argument**: `VERSION`"))]
	#[arg(value_name = "VERSION")]
	pub version: Version,

	/// Package to upgrade.
	///
	/// By default, this command detects which server is installed.
	///
	/// If both central and facility servers are present, it will error and you'll have to specify
	/// this option.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-k, --kind central|facility`"))]
	#[arg(short, long)]
	pub kind: Option<ApiServerKind>,

	/// Force installing older Tamanu
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--force-downgrade`"))]
	#[arg(long)]
	pub force_downgrade: bool,
}

pub async fn run(ctx: Context<TamanuArgs, PrepareUpgradeArgs>) -> Result<()> {
	let PrepareUpgradeArgs {
		version: new_version,
		kind,
		force_downgrade,
	} = ctx.args_sub;

	let (_, root) = find_tamanu(&ctx.args_top)?;
	let existing_version = find_existing_version()?;

	let kind = kind.unwrap_or_else(|| find_package(&root));
	info!(?kind, "using");

	// Assumptions here are that `root` is already canonicalised and all Windows installations have an upper root that can house multiple versioned Tamanu roots.
	let upper_root = root
		.parent()
		.expect(r"the tamanu root isn't canonicalised, it's the root directory");
	let existing_root = upper_root.join(format!("release-v{existing_version}"));
	let new_root = upper_root.join(format!("release-v{new_version}"));
	let new_web_root = upper_root.join(format!("tamanu-web-{new_version}"));

	let minimum_version = Version::parse("2.0.0").unwrap();
	if existing_version < minimum_version || new_version < minimum_version {
		bail!("version is too low, bestool doesn't support Tamanu <2.0.0");
	}

	if new_version == existing_version {
		bail!("version {new_version} is already installed");
	}

	if !force_downgrade && (new_version < existing_version) {
		bail!("refusing to downgrade (from {existing_version} to {new_version}) without `--force-downgrade`");
	}

	let client = client().await?;

	if !new_root.exists() {
		let url = make_url(kind.into(), new_version.to_string())?;
		Download::new(client.clone(), url)
			.and_extract(PkgFmt::Tzstd, &upper_root)
			.await
			.into_diagnostic()?;
	}

	if !new_web_root.exists() {
		let url = make_url(ServerKind::Web, new_version.to_string())?;
		Download::new(client, url)
			.and_extract(PkgFmt::Tzstd, &upper_root)
			.await
			.into_diagnostic()?;
	}

	duct::cmd!("cmd", "/C", "yarn", "--prod")
		.dir(&new_root)
		.run()
		.into_diagnostic()
		.wrap_err("failed to run yarn")?;

	let config_path = ["packages", kind.package_name(), "config", "local.json5"]
		.iter()
		.collect::<PathBuf>();
	let existing_config = existing_root.join(&config_path);
	let new_config = new_root.join(&config_path);
	info!(config = ?existing_config.display(), "copying configs to the new Tamanu installation");
	fs::copy(existing_config, new_config).into_diagnostic()?;

	info!("checking the new version is runnable");
	duct::cmd!("node", "dist", "help")
		.dir(&new_root.join("packages").join(kind.package_name()))
		.run()
		.into_diagnostic()?;

	if has_non_deterministic_migrations(&existing_root, &new_root)? {
		warn!(
			"The upgrade may contain non-deterministic migrations: check that's what you expect."
		);
	}

	fs::File::create(new_root.join(UPGRADED_SIGNAL_NAME)).into_diagnostic()?;

	Ok(())
}

/// Checks if there's new migration since the last update.
fn has_non_deterministic_migrations(
	existing_root: impl AsRef<Path>,
	new_root: impl AsRef<Path>,
) -> Result<bool> {
	let re = Regex::new(r"NON_DETERMINISTIC\s*=\s*true").unwrap();
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
