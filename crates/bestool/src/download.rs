use std::{
	iter,
	net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
	num::{NonZeroU16, NonZeroU64},
	time::Duration,
};

use binstalk_downloader::remote::{Client, Url};
use hickory_resolver::{
	ConnectionProvider, Resolver,
	config::{ConnectionConfig, NameServerConfig, ResolverConfig},
	net::runtime::TokioRuntimeProvider,
};
use miette::{IntoDiagnostic, Result};
use tokio::{net::TcpStream, time::timeout};
use tracing::{debug, info, instrument};

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn reqwest_client() -> Result<reqwest::Client> {
	let mut builder = reqwest::Client::builder();
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

	builder.build().into_diagnostic()
}

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
		let hostname = match self {
			Self::Tools => "bestool-proxy-tools",
			Self::Servers => "bestool-proxy-servers",
			Self::Meta => return Vec::new(),
		};

		let dns_addrs: Vec<SocketAddr> = tailscale_resolver()
			.lookup_ip(hostname)
			.await
			.ok()
			.map(|addrs| addrs.iter().map(|ip| SocketAddr::new(ip, 443)).collect())
			.unwrap_or_default();
		if !dns_addrs.is_empty() {
			return dns_addrs;
		}

		let hardcoded = self.hardcoded_proxy_addrs();
		debug!(
			?self,
			?hardcoded,
			"tailscale DNS lookup empty, probing hardcoded proxy IPs"
		);
		if probe_tcp_reachable(&hardcoded).await {
			return hardcoded;
		}

		Vec::new()
	}

	/// Hardcoded tailscale IPs for the proxy hosts, used when tailscale DNS
	/// (100.100.100.100) is unreachable but the tailnet otherwise is.
	fn hardcoded_proxy_addrs(self) -> Vec<SocketAddr> {
		match self {
			// bestool-proxy-tools
			Self::Tools => vec![
				SocketAddr::new(IpAddr::V4(Ipv4Addr::new(100, 101, 191, 59)), 443),
				SocketAddr::new(
					IpAddr::V6(Ipv6Addr::new(
						0xfd7a, 0x115c, 0xa1e0, 0, 0, 0, 0x7d01, 0xbf3c,
					)),
					443,
				),
			],
			// bestool-proxy-servers
			Self::Servers => vec![
				SocketAddr::new(IpAddr::V4(Ipv4Addr::new(100, 80, 8, 4)), 443),
				SocketAddr::new(
					IpAddr::V6(Ipv6Addr::new(
						0xfd7a, 0x115c, 0xa1e0, 0, 0, 0, 0x5f01, 0x0808,
					)),
					443,
				),
			],
			Self::Meta => Vec::new(),
		}
	}
}

async fn probe_tcp_reachable(addrs: &[SocketAddr]) -> bool {
	for &addr in addrs {
		match timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await {
			Ok(Ok(_)) => return true,
			Ok(Err(err)) => debug!(?addr, %err, "tcp probe failed"),
			Err(_) => debug!(?addr, "tcp probe timed out"),
		}
	}
	false
}

fn tailscale_resolver() -> Resolver<impl ConnectionProvider> {
	Resolver::builder_with_config(
		ResolverConfig::from_parts(
			None,
			vec!["tail53aef.ts.net.".parse().unwrap()],
			vec![NameServerConfig::new(
				"100.100.100.100".parse().unwrap(),
				true,
				vec![ConnectionConfig::udp()],
			)],
		),
		TokioRuntimeProvider::default(),
	)
	.build()
	.expect("tailscale resolver config is hardcoded and cannot fail to build")
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
