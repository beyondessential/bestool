use std::path::PathBuf;

use clap::Parser;

use binstalk_downloader::download::{Download, PkgFmt};
use detect_targets::{TARGET, get_desired_targets};
use miette::{IntoDiagnostic, Result, miette};
use tracing::info;

use crate::download::{DownloadSource, client};

use super::Context;

/// Update this bestool.
///
/// Alias: self
#[derive(Debug, Clone, Parser)]
pub struct SelfUpdateArgs {
	/// Version to update to.
	#[arg(long, default_value = "latest")]
	pub version: String,

	/// Target to download.
	///
	/// Usually the auto-detected default is fine, in rare cases you may need to override it.
	#[arg(long)]
	pub target: Option<String>,

	/// Temporary directory to download to.
	///
	/// Defaults to the system temp directory.
	#[arg(long)]
	pub temp_dir: Option<PathBuf>,

	/// Add to the PATH (only on Windows).
	#[cfg(windows)]
	#[arg(short = 'P', long)]
	pub add_to_path: bool,
}

pub async fn run(ctx: Context<SelfUpdateArgs>) -> Result<()> {
	let SelfUpdateArgs {
		version,
		target,
		temp_dir,
		#[cfg(windows)]
		add_to_path,
	} = ctx.args_top;

	let client = client().await?;

	let detected_targets = get_desired_targets(target.map(|t| vec![t]));
	let detected_targets = detected_targets.get().await;

	let dir = temp_dir.unwrap_or_else(std::env::temp_dir);
	let filename = format!(
		"bestool{ext}",
		ext = if cfg!(windows) { ".exe" } else { "" }
	);
	let dest = dir.join(&filename);

	let host = DownloadSource::Tools.host();
	let url = host
		.join(&format!(
			"/bestool/{version}/{target}/{filename}",
			target = detected_targets
				.first()
				.cloned()
				.unwrap_or_else(|| TARGET.into()),
		))
		.into_diagnostic()?;
	info!(url = %url, "downloading");

	Download::new(client, url)
		.and_extract(PkgFmt::Bin, &dest)
		.await
		.into_diagnostic()?;

	#[cfg(windows)]
	if add_to_path && let Err(err) = add_self_to_path() {
		tracing::error!("{err:?}");
	}

	info!(?dest, "downloaded, self-upgrading");
	upgrade::run_upgrade(&dest, true, vec!["--version"])
		.map_err(|err| miette!("upgrade: {err:?}"))?;
	Ok(())
}

#[cfg(windows)]
fn add_self_to_path() -> Result<()> {
	let self_path = std::env::current_exe().into_diagnostic()?;
	let self_dir = self_path
		.parent()
		.ok_or_else(|| miette!("current exe is not in a dir?"))?;
	let self_dir = self_dir
		.to_str()
		.ok_or_else(|| miette!("current exe path is not utf-8"))?;

	windows_env::prepend("PATH", self_dir).into_diagnostic()?;

	Ok(())
}
