use std::{
	fmt::Debug,
	fs,
	path::{Path, PathBuf},
};

use clap::{Parser, Subcommand, ValueEnum};
use itertools::Itertools;
use miette::{IntoDiagnostic, Result, miette};
use node_semver::Version;
use tracing::{debug, instrument};

use crate::args::Args;

use super::Context;

mod connection_url;
mod roots;

/// Interact with Tamanu.
///
/// Alias: t
#[derive(Debug, Clone, Parser)]
pub struct TamanuArgs {
	/// Tamanu root to operate in
	#[arg(long)]
	pub root: Option<PathBuf>,

	/// Tamanu subcommand
	#[command(subcommand)]
	pub action: Action,

	#[doc(hidden)]
	#[arg(long, hide = true)]
	pub(crate) use_colours: bool,
}

super::subcommands! {
	[Context<Args, TamanuArgs> => {|ctx: Context<Args, TamanuArgs>| -> Result<(Action, Context<TamanuArgs>)> {
		let (top, mut ctx) = ctx.take_top();
		ctx.args_sub.use_colours = top.logging.color.enabled();
		Ok((ctx.args_sub.action.clone(), ctx.push(())))
	}}](with_sub)

	#[cfg(feature = "tamanu-alerts")]
	alerts => Alerts(AlertsArgs),
	#[cfg(feature = "tamanu-artifacts")]
	#[clap(alias = "art")]
	artifacts => Artifacts(ArtifactsArgs),
	#[cfg(feature = "tamanu-backup")]
	#[clap(alias = "b")]
	backup => Backup(BackupArgs),
	#[cfg(feature = "tamanu-backup-configs")]
	backup_configs => BackupConfigs(BackupConfigsArgs),
	#[cfg(feature = "tamanu-config")]
	#[clap(alias = "c")]
	config => Config(ConfigArgs),
	#[cfg(feature = "tamanu-url")]
	#[clap(aliases = ["db", "u", "url"])]
	db_url => DbUrl(DbUrlArgs),
	#[cfg(feature = "tamanu-download")]
	#[clap(aliases = ["d", "down"])]
	download => Download(DownloadArgs),
	#[cfg(feature = "tamanu-find")]
	find => Find(FindArgs),
	#[cfg(feature = "tamanu-greenmask")]
	greenmask_config => GreenmaskConfig(GreenmaskConfigArgs),
	#[cfg(feature = "tamanu-psql")]
	#[clap(aliases = ["p", "pg", "sql"])]
	psql => Psql(PsqlArgs)
}

/// What kind of server to interact with.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum ApiServerKind {
	/// Central server
	#[value(alias("central-server"))]
	Central,

	/// Facility server
	#[value(alias("facility-server"))]
	Facility,
}

impl ApiServerKind {
	pub fn package_name(&self) -> &'static str {
		match self {
			Self::Central => "central-server",
			Self::Facility => "facility-server",
		}
	}
}

#[instrument(level = "debug")]
pub fn find_tamanu(args: &TamanuArgs) -> Result<(Version, PathBuf)> {
	#[inline]
	fn inner(args: &TamanuArgs) -> Result<(Version, PathBuf)> {
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

	inner(args).inspect(|(version, root)| debug!(?root, ?version, "found Tamanu root"))
}

#[instrument(level = "debug")]
pub fn find_package(root: impl AsRef<Path> + Debug) -> ApiServerKind {
	fn inner(root: &Path) -> Result<ApiServerKind> {
		fs::read_dir(root.join("packages"))
			.into_diagnostic()?
			.filter_map_ok(|e| e.file_name().into_string().ok())
			.process_results(|mut iter| {
				iter.find_map(|dir_name| ApiServerKind::from_str(&dir_name, false).ok())
					.ok_or_else(|| miette!("Tamanu servers not found"))
			})
			.into_diagnostic()?
	}

	inner(root.as_ref())
		.inspect(|kind| debug!(?root, ?kind, "using this Tamanu for config"))
		.map_err(|err| debug!(?err, "failed to detect package, assuming facility"))
		.unwrap_or(ApiServerKind::Facility)
}
