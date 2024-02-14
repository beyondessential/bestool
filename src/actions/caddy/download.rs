use std::{
	iter,
	num::{NonZeroU16, NonZeroU64},
	path::PathBuf,
};

use binstalk_downloader::{
	download::{Download, PkgFmt},
	remote::{Client, Url},
};
use clap::Parser;
use detect_targets::get_desired_targets;
use miette::{bail, IntoDiagnostic, Result};
use tracing::info;

use crate::actions::Context;

use super::CaddyArgs;

/// Find Tamanu installations.
#[derive(Debug, Clone, Parser)]
pub struct DownloadArgs {
	/// Version to download.
	#[arg(value_name = "VERSION", default_value = "latest")]
	pub version: String,

	/// Where to download to.
	#[arg(long)]
	pub path: PathBuf,

	/// Print the URL, don't download.
	///
	/// Useful if you want to download it on a different machine, or with a different tool.
	#[arg(long)]
	pub url_only: bool,

	/// Target to download.
	///
	/// Usually the auto-detected default is fine, in rare cases you may need to override it.
	#[arg(long)]
	pub target: Option<String>,
}

pub async fn run(ctx: Context<CaddyArgs, DownloadArgs>) -> Result<()> {
	let DownloadArgs {
		version,
		path,
		url_only,
		target,
	} = ctx.args_sub;

	let detected_targets = get_desired_targets(target.map(|t| vec![t]));
	let detected_targets = detected_targets.get().await;

	let client = Client::new(
		crate::APP_NAME,
		None,
		NonZeroU16::new(1).unwrap(),
		NonZeroU64::new(1).unwrap(),
		iter::empty(),
	)
	.into_diagnostic()?;

	let mut url = None;
	for target in detected_targets {
		let try_url = Url::parse(&format!(
			"https://tools.ops.tamanu.io/caddy/{version}/caddy-{target}{ext}",
			ext = if target.contains("windows") {
				".exe"
			} else {
				""
			},
		))
		.into_diagnostic()?;
		info!(url=%try_url, "trying URL");
		if client
			.remote_gettable(try_url.clone())
			.await
			.into_diagnostic()?
		{
			url.replace((target, try_url));
			break;
		}
	}

	let Some((target, url)) = url else {
		bail!(
			"no valid URL found for caddy {} on {}",
			version,
			detected_targets.join(", ")
		);
	};

	if url_only {
		println!("{}", url);
		return Ok(());
	}

	if !path.exists() {
		info!(?path, "creating directory");
		std::fs::create_dir_all(&path).into_diagnostic()?;
	}

	info!(%url, "downloading");
	Download::new(client, url.clone())
		.and_extract(
			PkgFmt::Bin,
			path.join(format!(
				"caddy{}",
				if target.contains("windows") {
					".exe"
				} else {
					""
				}
			)),
		)
		.await
		.into_diagnostic()?;

	Ok(())
}
