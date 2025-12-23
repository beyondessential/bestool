use std::{
	iter,
	net::SocketAddr,
	num::{NonZeroU16, NonZeroU64},
};

use binstalk_downloader::remote::{Client, Url};
use hickory_resolver::{
	Resolver,
	config::{NameServerConfig, ResolverConfig},
	name_server::{ConnectionProvider, TokioConnectionProvider},
};
use miette::{IntoDiagnostic, Result};
use tracing::{debug, info, instrument};

pub async fn client() -> Result<Client> {
	let mut builder = Client::default_builder(crate::APP_NAME, None, &mut iter::empty());
	for source in [
		DownloadSource::Tools,
		DownloadSource::Servers,
		DownloadSource::Meta,
	] {
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
	Meta,
}

impl DownloadSource {
	pub fn host(self) -> Url {
		Url::parse(match self {
			Self::Tools => "https://tools.ops.tamanu.io",
			Self::Servers => "https://servers.ops.tamanu.io",
			Self::Meta => "https://meta.tamanu.app",
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
		// - we're querying tailscale DNS server directly
		// - we don't really want to have this be easily hijacked by another tailnet
		// this does have the effect of exposing our tailnet suffix here, but that should be safe
		tailscale_resolver()
			.lookup_ip(match self {
				Self::Tools => "bestool-proxy-tools",
				Self::Servers => "bestool-proxy-servers",
				Self::Meta => "tamanu-meta-prod-disabled",
			})
			.await
			.ok()
			.map(|addrs| addrs.iter().map(|ip| SocketAddr::new(ip, 443)).collect())
			.unwrap_or_default()
	}
}

fn tailscale_resolver() -> Resolver<impl ConnectionProvider> {
	Resolver::builder_with_config(
		ResolverConfig::from_parts(
			None,
			vec!["tail53aef.ts.net.".parse().unwrap()],
			vec![NameServerConfig::new(
				"100.100.100.100:53".parse().unwrap(),
				hickory_resolver::proto::xfer::Protocol::Udp,
			)],
		),
		TokioConnectionProvider::default(),
	)
	.build()
}

pub async fn check_for_update() -> Result<()> {
	let current_version = env!("CARGO_PKG_VERSION");

	let url = DownloadSource::Tools
		.host()
		.join("/bestool/latest-version.txt")
		.into_diagnostic()?;
	debug!(?url, "Checking for updates");

	let response = client()
		.await?
		.get(url)
		.send(true)
		.await
		.into_diagnostic()?;

	let latest_version = response.bytes().await.into_diagnostic()?;
	let latest_version = std::str::from_utf8(&latest_version).into_diagnostic()?;
	let latest_version = latest_version.trim();
	debug!(
		current = current_version,
		latest = latest_version,
		"Version check result"
	);

	if latest_version != current_version {
		info!(
			current = current_version,
			latest = latest_version,
			"A new version of bestool is available. Run 'bestool self-update' to update."
		);
	} else {
		debug!("No update available");
	}

	Ok(())
}
