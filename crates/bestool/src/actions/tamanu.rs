use std::{
	ffi::OsString,
	fmt::Debug,
	fs,
	path::{Path, PathBuf},
};

use clap::{Parser, Subcommand, ValueEnum};
use itertools::Itertools;
use miette::{IntoDiagnostic, Result, miette};
use node_semver::Version;
use tracing::{debug, instrument};

use super::Context;

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
}

super::subcommands! {
	[Context<TamanuArgs> => {|ctx: Context<TamanuArgs>| -> Result<(Action, Context<TamanuArgs>)> {
		Ok((ctx.args_top.action.clone(), ctx.with_sub(())))
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

#[cfg(feature = "tamanu-pg-common")]
#[instrument(level = "debug")]
pub fn find_postgres_bin(name: &str) -> Result<OsString> {
	use std::env;

	#[allow(dead_code)]
	#[tracing::instrument(level = "debug")]
	fn find_from_installation(root: &str, name: &str) -> Result<OsString> {
		let version = fs::read_dir(root)
			.into_diagnostic()?
			.filter_map(|res| {
				res.map(|dir| {
					dir.file_name()
						.into_string()
						.ok()
						.filter(|name| name.parse::<u32>().is_ok())
				})
				.transpose()
			})
			// Use `u32::MAX` in case of `Err` so that we always catch IO errors.
			.max_by_key(|res| {
				res.as_ref()
					.cloned()
					.map(|n| n.parse::<u32>().unwrap())
					.unwrap_or(u32::MAX)
			})
			.ok_or_else(|| miette!("the Postgres root {root} is empty"))?
			.into_diagnostic()?;

		let exec_file_name = if cfg!(windows) {
			format!("{name}.exe")
		} else {
			name.to_string()
		};
		Ok([root, version.as_str(), "bin", &exec_file_name]
			.iter()
			.collect::<PathBuf>()
			.into())
	}

	#[allow(dead_code)]
	fn is_in_path(name: &str) -> Option<PathBuf> {
		let var = env::var_os("PATH")?;

		// Separate PATH value into paths
		let paths_iter = env::split_paths(&var);

		// Attempt to read each path as a directory
		let dirs_iter = paths_iter.filter_map(|path| fs::read_dir(path).ok());

		for dir in dirs_iter {
			let mut matches_iter = dir
				.filter_map(|file| file.ok())
				.filter(|file| file.file_name() == name);
			if let Some(file) = matches_iter.next() {
				return Some(file.path());
			}
		}

		None
	}

	// On Windows, find `psql` assuming the standard installation using the installer
	// because PATH on Windows is not reliable.
	// See https://github.com/rust-lang/rust/issues/37519
	#[cfg(windows)]
	return find_from_installation(r"C:\Program Files\PostgreSQL", name);

	#[cfg(target_os = "linux")]
	if is_in_path(name).is_some() {
		Ok(name.into())
	} else {
		// Ubuntu recommends to use pg_ctlcluster over pg_ctl and doesn't put pg_ctl in PATH.
		// Still, it should be fine for temporary database.
		find_from_installation(r"/usr/lib/postgresql", name)
	}

	#[cfg(not(any(windows, target_os = "linux")))]
	return Ok(name.into());
}
