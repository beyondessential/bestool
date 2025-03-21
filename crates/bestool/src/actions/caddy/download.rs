use std::path::PathBuf;

use binstalk_downloader::download::{Download, PkgFmt};
use clap::Parser;
use detect_targets::get_desired_targets;
use miette::{bail, IntoDiagnostic, Result};
use tracing::{debug, info};

use crate::{
	actions::Context,
	download::{client, DownloadSource},
};

use super::CaddyArgs;

/// Download caddy.
#[cfg_attr(docsrs, doc("\n\n**Command**: `bestool caddy download`"))]
#[derive(Debug, Clone, Parser)]
pub struct DownloadArgs {
	/// Version to download.
	#[cfg_attr(docsrs, doc("\n\n**Argument**: version, default `latest`"))]
	#[arg(value_name = "VERSION", default_value = "latest")]
	pub version: String,

	/// Where to download to.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--path PATH`"))]
	#[arg(long)]
	pub path: PathBuf,

	/// Print the URL, don't download.
	///
	/// Useful if you want to download it on a different machine, or with a different tool.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--url-only`"))]
	#[arg(long)]
	pub url_only: bool,

	/// Target to download.
	///
	/// Usually the auto-detected default is fine, in rare cases you may need to override it.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--target TARGET`"))]
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

	let client = client().await?;

	let host = DownloadSource::Tools.host();

	let mut url = None;
	for target in detected_targets {
		let try_url = host
			.join(&format!(
				"/caddy/{version}/caddy-{target}{ext}?bust={date}",
				ext = if target.contains("windows") {
					".exe"
				} else {
					""
				},
				date = chrono::Utc::now(),
			))
			.into_diagnostic()?;
		debug!(url=%try_url, "trying URL");
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
		debug!(?path, "creating directory");
		std::fs::create_dir_all(&path).into_diagnostic()?;
	}

	let fullpath = path.join(format!(
		"caddy{}",
		if target.contains("windows") {
			".exe"
		} else {
			""
		}
	));

	info!(%url, path=?fullpath, "downloading");
	Download::new(client, url.clone())
		.and_extract(PkgFmt::Bin, &fullpath)
		.await
		.into_diagnostic()?;

	#[cfg(unix)]
	if !target.contains("windows") {
		use std::{
			fs::{set_permissions, Permissions},
			os::unix::fs::PermissionsExt,
		};
		info!(path=?fullpath, "marking as executable");
		set_permissions(&fullpath, Permissions::from_mode(0o755)).into_diagnostic()?;
	}

	Ok(())
}
