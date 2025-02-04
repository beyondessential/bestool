use std::{
	iter,
	net::SocketAddr,
	num::{NonZeroU16, NonZeroU64},
};

use binstalk_downloader::remote::{Client, Url};
use miette::{IntoDiagnostic, Result};
use tokio::net::lookup_host;
use tracing::{debug, instrument};

pub async fn client() -> Result<Client> {
	let mut builder = Client::default_builder(crate::APP_NAME, None, &mut iter::empty());
	for source in [DownloadSource::Tools, DownloadSource::Servers] {
		let addrs = source.source_alternatives().await;
		if !addrs.is_empty() {
			debug!(
				?source,
				?addrs,
				"using alternative addresses for a download source"
			);
			builder = builder.resolve_to_addrs(&source.domain(), &addrs);
		}
	}

	Client::from_builder(
		builder,
		NonZeroU16::new(1).unwrap(),
		NonZeroU64::new(1).unwrap(),
	)
	.into_diagnostic()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DownloadSource {
	Tools,
	Servers,
}

impl DownloadSource {
	pub fn host(self) -> Url {
		Url::parse(match self {
			Self::Tools => "https://tools.ops.tamanu.io",
			Self::Servers => "https://servers.ops.tamanu.io",
		})
		.unwrap()
	}

	pub fn domain(self) -> String {
		self.host().host_str().unwrap().to_owned()
	}

	#[instrument(level = "TRACE")]
	async fn source_alternatives(self) -> Vec<SocketAddr> {
		// tailscale proxies, if available, can bypass outbound firewalls
		// need to use the full name because:
		// - in some cases on windows, the short name is not resolved
		// - we don't really want to have this be easily hijacked by another tailnet
		// this does have the effect of exposing our tailnet suffix here, but that should be safe
		lookup_host(match self {
			Self::Tools => "bestool-proxy-tools.tail53aef.ts.net:443",
			Self::Servers => "bestool-proxy-servers.tail53aef.ts.net:443",
		})
		.await
		.ok()
		.map(|addrs| addrs.collect())
		.unwrap_or_default()
	}
}
