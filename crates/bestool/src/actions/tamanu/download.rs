use std::path::{Path, PathBuf};

use binstalk_downloader::download::{Download, PkgFmt};
use clap::Parser;
use miette::{bail, IntoDiagnostic, Result};

use crate::{
	actions::{
		tamanu::artifacts::{get_artifacts, Platform},
		Context,
	},
	download::client,
};

use super::TamanuArgs;

/// Download Tamanu artifacts.
///
/// Use the `tamanu artifacts` subcommand to list of the artifacts available for a version.
///
/// Aliases: d, down
#[derive(Debug, Clone, Parser)]
pub struct DownloadArgs {
	/// Artifact type to download.
	///
	/// You can find the artifact list using the `tamanu artifacts` subcommand.
	///
	/// For backward compatibility, `web` is an alias to `frontend`, and `facility-server` /
	/// `central-server` are aliases to `facility` / `central`. Prefer the literal values.
	#[arg(value_name = "ARTIFACT TYPE")]
	pub kind: String,

	/// Version to download.
	#[arg(value_name = "VERSION")]
	pub version: String,

	/// Where to download to.
	#[arg(long)]
	#[cfg_attr(windows, arg(default_value = "/Tamanu"))]
	#[cfg_attr(not(windows), arg(default_value = "."))]
	pub into: PathBuf,

	/// Print the URL, don't download.
	///
	/// Useful if you want to download it on a different machine, or with a different tool.
	#[arg(long)]
	pub url_only: bool,

	/// Don't extract (if the download is an archive).
	#[arg(long)]
	pub no_extract: bool,

	/// Platform to download artifacts for.
	///
	/// Use `host` (default) for the auto-detected current platform, `container` for container artifacts,
	/// `os-arch` for specific targets (e.g., `linux-x86_64`), and `all` to list all platforms.
	///
	/// This is mostly useful with `--url-only` or `--no-extract`.
	#[arg(short, long, value_name = "PLATFORM", default_value = "host")]
	pub platform: Platform,
}

pub async fn run(ctx: Context<TamanuArgs, DownloadArgs>) -> Result<()> {
	let DownloadArgs {
		kind,
		version,
		into,
		url_only,
		no_extract,
		platform,
	} = ctx.args_sub;

	if platform == Platform::Match("container".into()) {
		bail!("Cannot download container artifacts");
	}

	let artifact_type = match kind.to_ascii_lowercase().as_str() {
		"web" => "frontend".to_string(),
		"central-server" => "central".to_string(),
		"facility-server" => "facility".to_string(),
		other => other.to_string(),
	};

	let Some(artifact) = get_artifacts(&version, &platform)
		.await?
		.into_iter()
		.find(|artifact| artifact.artifact_type == artifact_type)
	else {
		bail!("No such artifact found");
	};

	if url_only {
		println!("{}", artifact.download_url);
		return Ok(());
	}

	let fmt = if no_extract {
		PkgFmt::Bin
	} else if let Some(fmt) = PkgFmt::guess_pkg_format(artifact.download_url.as_str()) {
		fmt
	} else {
		PkgFmt::Bin
	};

	let path = if into.is_dir() && fmt == PkgFmt::Bin {
		let file = into.join(format!(
			"tamanu-{}-{}{}",
			artifact.artifact_type,
			version,
			Path::new(artifact.download_url.path()).extension().map_or(
				Default::default(),
				|ext| format!(".{}", ext.to_string_lossy())
			)
		));
		if file.exists() {
			bail!("{file:?} already exists, refusing to overwrite");
		}
		file
	} else {
		into
	};

	let client = client().await?;

	eprintln!("Downloading {} to {path:?}...", artifact.download_url);
	Download::new(client, artifact.download_url)
		.and_extract(fmt, &path)
		.await
		.into_diagnostic()?;
	Ok(())
}
