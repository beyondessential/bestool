use std::{
	iter,
	num::{NonZeroU16, NonZeroU64},
	path::PathBuf,
};

use clap::Parser;

use binstalk_downloader::{
	download::{Download, PkgFmt},
	remote::{Client, Url},
};
use detect_targets::{get_desired_targets, TARGET};
use miette::{IntoDiagnostic, Result};
use tracing::info;

use super::Context;

/// Update this bestool.
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
}

pub async fn run(ctx: Context<SelfUpdateArgs>) -> Result<()> {
	let SelfUpdateArgs {
		version,
		target,
		temp_dir,
	} = ctx.args_top;

	let client = Client::new(
		crate::APP_NAME,
		None,
		NonZeroU16::new(1).unwrap(),
		NonZeroU64::new(1).unwrap(),
		iter::empty(),
	)
	.into_diagnostic()?;

	let detected_targets = get_desired_targets(target.map(|t| vec![t]));
	let detected_targets = detected_targets.get().await;

	let dir = temp_dir.unwrap_or_else(std::env::temp_dir);
	let filename = format!(
		"bestool{ext}",
		ext = if cfg!(windows) { ".exe" } else { "" }
	);
	let dest = dir.join(&filename);

	let url = format!(
		"https://tools.ops.tamanu.io/bestool/{version}/{target}/{filename}",
		target = detected_targets
			.first()
			.cloned()
			.unwrap_or_else(|| TARGET.into()),
	);
	info!(url = %url, "downloading");

	Download::new(client, Url::parse(&url).into_diagnostic()?)
		.and_extract(PkgFmt::Bin, &dest)
		.await
		.into_diagnostic()?;

	info!(?dest, "downloaded, self-updating");
	self_replace::self_replace(&dest).into_diagnostic()?;
	std::fs::remove_file(&dest).into_diagnostic()?;

	Ok(())
}
