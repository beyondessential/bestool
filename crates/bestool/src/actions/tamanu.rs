use std::{
	fs,
	path::{Path, PathBuf},
};

use clap::{Parser, Subcommand, ValueEnum};
use itertools::Itertools;
use miette::{miette, IntoDiagnostic, Result};
use node_semver::Version;

use super::Context;

mod roots;

/// Interact with Tamanu.
#[derive(Debug, Clone, Parser)]
pub struct TamanuArgs {
	/// Tamanu root to operate in
	#[arg(long)]
	pub root: Option<PathBuf>,

	/// Tamanu subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[Context<TamanuArgs> => {|ctx: Context<TamanuArgs>| -> Result<(Action, Context<TamanuArgs>)> {
		Ok((ctx.args_top.action.clone(), ctx.with_sub(())))
	}}](with_sub)

	#[cfg(feature = "tamanu-alerts")]
	alerts => Alerts(AlertsArgs),
	#[cfg(feature = "tamanu-config")]
	config => Config(ConfigArgs),
	#[cfg(feature = "tamanu-download")]
	download => Download(DownloadArgs),
	#[cfg(feature = "tamanu-find")]
	find => Find(FindArgs),
	#[cfg(all(windows, feature = "tamanu-upgrade"))]
	prepare_upgrade => PrepareUpgrade(PrepareUpgradeArgs),
	#[cfg(feature = "tamanu-psql")]
	psql => Psql(PsqlArgs),
	#[cfg(all(windows, feature = "tamanu-upgrade"))]
	upgrade => Upgrade(UpgradeArgs)
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

pub fn find_package(root: impl AsRef<Path>) -> Result<ApiServerKind> {
	fs::read_dir(root.as_ref().join("packages"))
		.into_diagnostic()?
		.filter_map_ok(|e| e.file_name().into_string().ok())
		.process_results(|mut iter| {
			iter.find_map(|dir_name| ApiServerKind::from_str(&dir_name, false).ok())
				.ok_or_else(|| miette!("Tamanu servers not found"))
		})
		.into_diagnostic()?
}

#[cfg(windows)]
pub fn find_existing_version() -> Result<Version> {
	use miette::WrapErr;

	#[derive(serde::Deserialize, Debug)]
	struct Process {
		name: String,
		pm2_env: Pm2Env,
	}

	#[derive(serde::Deserialize, Debug)]
	struct Pm2Env {
		version: Version,
	}

	let reader = duct::cmd!("cmd", "/C", "pm2", "jlist")
		.reader()
		.into_diagnostic()
		.wrap_err("failed to run pm2")?;
	let processes: Vec<Process> = serde_json::from_reader(reader).into_diagnostic()?;

	Ok(processes
		.into_iter()
		.find(|p| p.name == "tamanu-api-server" || p.name == "tamanu-http-server")
		.ok_or_else(|| miette!("there's no live Tamanu running"))?
		.pm2_env
		.version)
}
