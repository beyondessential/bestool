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
	let mut builder = crate::http::client_builder();
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
	let mut builder = Client::default_builder(crate::http::user_agent(), None, &mut iter::empty());
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

pub async fn fetch_latest_version() -> Result<String> {
	let url = DownloadSource::Tools
		.host()
		.join("/bestool/latest-version.txt")
		.into_diagnostic()?;
	debug!(?url, "Fetching latest bestool version");

	let response = client()
		.await?
		.get(url)
		.send(true)
		.await
		.into_diagnostic()?;

	let body = response.bytes().await.into_diagnostic()?;
	let latest = std::str::from_utf8(&body)
		.into_diagnostic()?
		.trim()
		.to_owned();
	Ok(latest)
}

pub async fn check_for_update() -> Result<()> {
	let current_version = env!("CARGO_PKG_VERSION");
	let latest_version = fetch_latest_version().await?;
	debug!(
		current = current_version,
		latest = %latest_version,
		"Version check result"
	);

	if remote_is_newer(current_version, &latest_version) {
		info!(
			current = current_version,
			latest = %latest_version,
			"A new version of bestool is available. Run 'bestool self-update' to update."
		);
	} else {
		debug!("No update available");
	}

	Ok(())
}

/// Trust anchor for released binaries: the minisign public key whose private
/// counterpart is held only by the release pipeline. It ships in the binary so
/// an update can be verified against a key that travelled with the running
/// build rather than one fetched at update time.
#[cfg(feature = "self-update")]
pub(crate) const RELEASE_PUBLIC_KEY: &str =
	"RWT+oj++Y0app3N4K+PLSYTKhtXimltIHxhoFgyWjxR/ZElCG0lDBDl5";

/// Fetch and decode the detached signature published next to `artifact_url`
/// (at the artifact URL with a `.minisig` suffix).
#[cfg(feature = "self-update")]
pub(crate) async fn fetch_release_signature(
	client: &Client,
	artifact_url: &Url,
) -> Result<minisign_verify::Signature> {
	use miette::miette;

	let sig_url = Url::parse(&format!("{artifact_url}.minisig")).into_diagnostic()?;
	debug!(%sig_url, "fetching release signature");

	let response = client
		.get(sig_url)
		.send(true)
		.await
		.map_err(|err| miette!("could not fetch release signature: {err}"))?;
	let sig_bytes = response.bytes().await.into_diagnostic()?;
	let sig_text = std::str::from_utf8(&sig_bytes)
		.map_err(|err| miette!("release signature is not UTF-8: {err}"))?;
	minisign_verify::Signature::decode(sig_text)
		.map_err(|err| miette!("malformed release signature: {err}"))
}

/// Streams a downloaded artifact through minisign verification against
/// [`RELEASE_PUBLIC_KEY`].
///
/// Plug it into the downloader with [`Download::new_with_data_verifier`], then
/// call [`DataVerifier::validate`] once the download finishes: it returns
/// `false` unless the bytes that streamed through carry a valid release
/// signature. The artifact verified is the exact one downloaded, so an
/// unverified artifact is rejected before its contents are trusted.
///
/// [`Download::new_with_data_verifier`]: binstalk_downloader::download::Download::new_with_data_verifier
#[cfg(feature = "self-update")]
pub(crate) struct ReleaseVerifier {
	signature: minisign_verify::Signature,
	data: Vec<u8>,
}

#[cfg(feature = "self-update")]
impl ReleaseVerifier {
	pub(crate) fn new(signature: minisign_verify::Signature) -> Self {
		Self {
			signature,
			data: Vec::new(),
		}
	}
}

#[cfg(feature = "self-update")]
impl binstalk_downloader::download::DataVerifier for ReleaseVerifier {
	fn update(&mut self, data: &binstalk_downloader::bytes::Bytes) {
		self.data.extend_from_slice(data);
	}

	fn validate(&mut self) -> bool {
		if verify_signed(RELEASE_PUBLIC_KEY, &self.signature, &self.data) {
			info!("release signature verified");
			true
		} else {
			false
		}
	}
}

/// Check `data` against `signature` under the base64 minisign public key
/// `public_key_b64`. Rejects the legacy (non-prehashed) minisign format. Logs
/// and returns `false` on any failure rather than erroring, for the
/// `DataVerifier::validate` contract.
#[cfg(feature = "self-update")]
fn verify_signed(
	public_key_b64: &str,
	signature: &minisign_verify::Signature,
	data: &[u8],
) -> bool {
	let public_key = match minisign_verify::PublicKey::from_base64(public_key_b64) {
		Ok(key) => key,
		Err(err) => {
			tracing::error!("invalid release public key: {err}");
			return false;
		}
	};
	match public_key.verify(data, signature, false) {
		Ok(()) => true,
		Err(err) => {
			tracing::error!("release signature did not verify: {err}");
			false
		}
	}
}

/// Whether the remote version is strictly higher than the current version.
///
/// Avoids notifying when a dev or pre-release build (e.g. installed from a
/// branch) happens to be ahead of the published release. If either side can't
/// be parsed as semver, falls back to string inequality so we still surface
/// *something* — a parse failure shouldn't mask a real available update.
pub(crate) fn remote_is_newer(current: &str, latest: &str) -> bool {
	match (
		semver::Version::parse(current),
		semver::Version::parse(latest),
	) {
		(Ok(c), Ok(l)) => l > c,
		_ => current != latest,
	}
}

#[cfg(test)]
mod tests {
	use super::remote_is_newer;

	#[test]
	fn remote_newer_when_remote_is_higher_patch() {
		assert!(remote_is_newer("1.10.0", "1.10.1"));
	}

	#[test]
	fn remote_newer_when_remote_is_higher_minor() {
		assert!(remote_is_newer("1.10.5", "1.11.0"));
	}

	#[test]
	fn not_newer_when_equal() {
		assert!(!remote_is_newer("1.10.0", "1.10.0"));
	}

	#[test]
	fn not_newer_when_local_is_ahead() {
		// The case the TODO calls out: a dev build that's ahead of the
		// published release shouldn't trigger an "update available" notice.
		assert!(!remote_is_newer("1.12.0", "1.11.0"));
	}

	#[test]
	fn double_digit_components_compared_numerically_not_lexically() {
		// String comparison would say "1.9.0" > "1.10.0" — semver knows better.
		assert!(remote_is_newer("1.9.0", "1.10.0"));
		assert!(!remote_is_newer("1.10.0", "1.9.0"));
	}

	#[test]
	fn unparseable_falls_back_to_inequality() {
		assert!(remote_is_newer("not-semver", "1.0.0"));
		assert!(!remote_is_newer("not-semver", "not-semver"));
	}
}

#[cfg(all(test, feature = "self-update"))]
mod signature_tests {
	use std::io::Cursor;

	use minisign_verify::Signature;

	use super::{RELEASE_PUBLIC_KEY, verify_signed};

	/// Sign `data` with a fresh keypair, returning (public-key base64, signature
	/// text). `minisign::sign` produces a prehashed signature by default — the
	/// only format the verifier accepts.
	fn sign(data: &[u8]) -> (String, String) {
		let keypair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
		let signature = minisign::sign(None, &keypair.sk, Cursor::new(data), None, None).unwrap();
		(keypair.pk.to_base64(), signature.into_string())
	}

	#[test]
	fn embedded_release_key_is_a_valid_minisign_key() {
		assert!(minisign_verify::PublicKey::from_base64(RELEASE_PUBLIC_KEY).is_ok());
	}

	#[test]
	fn embedded_release_key_matches_binstall_metadata() {
		// self-update (RELEASE_PUBLIC_KEY) and binstall (the
		// [package.metadata.binstall.signing] pubkey) verify against the same
		// released artifacts, so they must be the same key. Keeping both copies
		// in step is otherwise a silent, release-only failure.
		let manifest = include_str!("../Cargo.toml");
		let expected = format!("pubkey = \"{RELEASE_PUBLIC_KEY}\"");
		assert!(
			manifest.contains(&expected),
			"RELEASE_PUBLIC_KEY must match the binstall signing pubkey in Cargo.toml"
		);
	}

	#[test]
	fn accepts_a_valid_signature() {
		let data = b"the bestool release archive bytes";
		let (pubkey, sig) = sign(data);
		let signature = Signature::decode(&sig).unwrap();
		assert!(verify_signed(&pubkey, &signature, data));
	}

	#[test]
	fn rejects_tampered_data() {
		let (pubkey, sig) = sign(b"the bestool release archive bytes");
		let signature = Signature::decode(&sig).unwrap();
		assert!(!verify_signed(
			&pubkey,
			&signature,
			b"tampered archive bytes"
		));
	}

	#[test]
	fn rejects_a_signature_from_a_different_key() {
		let data = b"the bestool release archive bytes";
		let (_pubkey, sig) = sign(data);
		let signature = Signature::decode(&sig).unwrap();
		let (other_pubkey, _) = sign(data);
		assert!(!verify_signed(&other_pubkey, &signature, data));
	}
}
