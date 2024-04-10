use std::{
	iter,
	num::{NonZeroU16, NonZeroU64},
	path::{Path, PathBuf},
};

use binstalk_downloader::{
	download::{Download, PkgFmt},
	remote::{Client, Url},
};
use clap::{Parser, ValueEnum};
use miette::{IntoDiagnostic, Result};

use crate::actions::Context;

use super::{ServerKind, TamanuArgs};

/// Find Tamanu installations.
#[derive(Debug, Clone, Parser)]
pub struct DownloadArgs {
	/// What to download.
	#[arg(value_name = "KIND")]
	pub kind: ApiServerKind,

	/// Version to download.
	#[arg(value_name = "VERSION")]
	pub version: String,

	/// Where to download to.
	#[arg(long, default_value = "/Tamanu")]
	pub into: PathBuf,

	/// Print the URL, don't download.
	///
	/// Useful if you want to download it on a different machine, or with a different tool.
	#[arg(long)]
	pub url_only: bool,
}

/// What kind of server to download.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum ApiServerKind {
	/// Central server
	Central,

	/// Facility server
	Facility,

	/// Web frontend
	Web,
}

impl From<ServerKind> for ApiServerKind {
	fn from(value: ServerKind) -> Self {
		match value {
			ServerKind::Central => Self::Central,
			ServerKind::Facility => Self::Facility,
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

	download(url, into).await
}

pub fn make_url(kind: ApiServerKind, version: String) -> Result<Url> {
	let url_string = format!(
		"https://servers.ops.tamanu.io/{version}/{kind}-{version}{platform}.tar.zst",
		kind = match kind {
			ApiServerKind::Central => "central",
			ApiServerKind::Facility => "facility",
			ApiServerKind::Web => "web",
		},
		platform = if kind == ApiServerKind::Web {
			""
		} else {
			"-windows"
		},
	);

	Url::parse(&url_string).into_diagnostic()
}

pub async fn download(url: Url, into: impl AsRef<Path>) -> Result<()> {
	let client = Client::new(
		crate::APP_NAME,
		None,
		NonZeroU16::new(1).unwrap(),
		NonZeroU64::new(1).unwrap(),
		iter::empty(),
	)
	.into_diagnostic()?;
	let download = Download::new(client, url);
	download
		.and_extract(PkgFmt::Tzstd, into)
		.await
		.into_diagnostic()?;

	Ok(())
}
