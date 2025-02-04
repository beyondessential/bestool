use std::path::PathBuf;

use binstalk_downloader::{
	download::{Download, PkgFmt},
	remote::Url,
};
use clap::{Parser, ValueEnum};
use miette::{IntoDiagnostic, Result};

use crate::{
	actions::Context,
	download::{client, DownloadSource},
};

use super::{ApiServerKind, TamanuArgs};

/// Download Tamanu servers.
///
/// In general, you should prefer to use the container images.
/// This command is here to support Windows deployments, which run servers with a system Node.js.
/// It will be deprecated in the future as Windows containers are developed for Tamanu.
#[cfg_attr(docsrs, doc("\n\n**Command**: `bestool tamanu download`"))]
#[derive(Debug, Clone, Parser)]
pub struct DownloadArgs {
	/// What to download.
	#[cfg_attr(docsrs, doc("\n\n**1st Argument**: `central|facility|web`"))]
	#[arg(value_name = "KIND")]
	pub kind: ServerKind,

	/// Version to download.
	#[cfg_attr(
		docsrs,
		doc("\n\n**2nd Argument**: version (e.g. `bestool tamanu download web 1.2.3`)")
	)]
	#[arg(value_name = "VERSION")]
	pub version: String,

	/// Where to download to.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--into PATH`, default C:\\Tamanu"))]
	#[arg(long, default_value = "/Tamanu")]
	pub into: PathBuf,

	/// Print the URL, don't download.
	///
	/// Useful if you want to download it on a different machine, or with a different tool.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--url-only`"))]
	#[arg(long)]
	pub url_only: bool,
}

/// What kind of server to download.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum ServerKind {
	/// Central server
	Central,

	/// Facility server
	Facility,

	/// Web frontend
	Web,
}

impl From<ApiServerKind> for ServerKind {
	fn from(value: ApiServerKind) -> Self {
		match value {
			ApiServerKind::Central => Self::Central,
			ApiServerKind::Facility => Self::Facility,
		}
	}
}

pub async fn run(ctx: Context<TamanuArgs, DownloadArgs>) -> Result<()> {
	let DownloadArgs {
		kind,
		version,
		into,
		url_only,
	} = ctx.args_sub;

	let url = make_url(kind, version)?;

	if url_only {
		println!("{}", url);
		return Ok(());
	}

	let client = client().await?;
	Download::new(client, url)
		.and_extract(PkgFmt::Tzstd, &into)
		.await
		.into_diagnostic()?;
	Ok(())
}

pub fn make_url(kind: ServerKind, version: String) -> Result<Url> {
	let host = DownloadSource::Servers.host();
	host.join(&format!(
		"/{version}/{kind}-{version}{platform}.tar.zst",
		kind = match kind {
			ServerKind::Central => "central",
			ServerKind::Facility => "facility",
			ServerKind::Web => "web",
		},
		platform = if kind == ServerKind::Web {
			""
		} else {
			"-windows"
		},
	))
	.into_diagnostic()
}
