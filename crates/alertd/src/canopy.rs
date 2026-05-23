use std::{
	fmt,
	io::Write,
	net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
	time::Duration,
};

use flate2::{Compression, write::GzEncoder};
use hickory_resolver::{
	ConnectionProvider, Resolver,
	config::{ConnectionConfig, NameServerConfig, ResolverConfig},
	net::runtime::TokioRuntimeProvider,
};
use jiff::Timestamp;
use miette::{IntoDiagnostic, Result, WrapErr};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::sync::RwLock;
use tracing::debug;

use crate::Redacted;

pub const DEFAULT_CANOPY_URL: &str = "https://meta.tamanu.app";

/// Base URL for the tailscale-internal canopy endpoint.
///
/// On hosts that share the canopy tailnet, posting to this URL works without
/// mTLS — the tailscale identity is the auth.
pub const TAILSCALE_URL: &str = "https://canopy.tail53aef.ts.net";

/// Bare hostname used for `resolve_to_addrs` overrides.
const TAILSCALE_HOST: &str = "canopy.tail53aef.ts.net";

/// Hardcoded tailscale IPs for canopy, used when tailscale DNS
/// (100.100.100.100) is unreachable but the tailnet otherwise is.
const CANOPY_HARDCODED_V4: Ipv4Addr = Ipv4Addr::new(100, 99, 98, 97);
const CANOPY_HARDCODED_V6: Ipv6Addr =
	Ipv6Addr::new(0xfd7a, 0x115c, 0xa1e0, 0, 0, 0, 0x9337, 0xfb52);

/// How long renewed canopy certs are valid for.
///
/// Set well above [`CERT_RENEW_AFTER`] so a renewal failure doesn't immediately
/// strand the client.
const CERT_VALIDITY_DAYS: i64 = 6;

/// How long to wait between scheduled cert renewals.
///
/// Renewal runs in a background task in the daemon; the legacy single-shot
/// alerts command builds the client once and exits well within this window.
pub const CERT_RENEW_AFTER: Duration = Duration::from_secs(5 * 24 * 60 * 60);

/// Timeout for the tailscale availability probe.
const TAILSCALE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// RFC 5424 syslog severities accepted by the canopy `/events` API.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
	Emergency,
	Alert,
	Critical,
	Error,
	Warning,
	Notice,
	Info,
	Debug,
}

/// Payload for posting to `POST /events` on a canopy server.
#[derive(Debug, Clone, Serialize)]
pub struct NewEvent<'a> {
	pub source: &'a str,
	#[serde(rename = "ref")]
	pub r#ref: &'a str,
	pub message: &'a str,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub description: Option<&'a str>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub severity: Option<Severity>,
	#[serde(rename = "occurredAt", skip_serializing_if = "Option::is_none")]
	pub occurred_at: Option<Timestamp>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub active: Option<bool>,
}

/// HTTP client with auth configured for talking to a canopy server.
///
/// Tries two auth paths in order of preference:
/// 1. **Tailscale**: if the canopy tailnet endpoint is reachable, plain HTTPS
///    works (auth is implicit via tailscale identity).
/// 2. **mTLS**: a fresh self-signed cert from the device key, short-lived
///    ([`CERT_VALIDITY_DAYS`]); for long-running daemons, [`Self::renew`]
///    should tick on [`CERT_RENEW_AFTER`] to swap in a fresh cert before expiry.
///
/// [`Self::refresh`] re-probes tailscale and swaps modes on reload.
pub struct CanopyClient {
	device_key: Option<Redacted<String>>,
	/// Tamanu version of the install this client speaks for. Sent verbatim in
	/// the `X-Version` request header — canopy rejects events / status pushes
	/// that don't carry one. Sourced from the running Tamanu install's
	/// `package.json` (via `find_tamanu`); not the bestool / alertd version.
	tamanu_version: String,
	state: RwLock<State>,
}

enum State {
	Tailscale(reqwest::Client),
	Mtls(reqwest::Client),
}

impl State {
	fn is_tailscale(&self) -> bool {
		matches!(self, State::Tailscale(_))
	}

	fn http(&self) -> reqwest::Client {
		match self {
			State::Tailscale(http) | State::Mtls(http) => http.clone(),
		}
	}
}

impl fmt::Debug for CanopyClient {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CanopyClient").finish_non_exhaustive()
	}
}

impl CanopyClient {
	/// Build a canopy client, preferring tailscale and falling back to mTLS.
	///
	/// Probes the tailscale canopy endpoint first; if reachable, uses it.
	/// Otherwise, if a device key PEM is provided, builds an mTLS client.
	/// Returns `Ok(None)` if neither path is available.
	///
	/// `tamanu_version` is the version of the Tamanu install this client
	/// speaks for; sent on every request via the `X-Version` header.
	pub async fn new(
		tamanu_version: impl Into<String>,
		device_key_pem: Option<&str>,
	) -> Result<Option<Self>> {
		let tamanu_version = tamanu_version.into();
		let device_key = device_key_pem.map(|s| Redacted(s.to_owned()));

		if let Some(http) = probe_tailscale().await {
			debug!("canopy: tailscale endpoint reachable, preferring it");
			return Ok(Some(Self {
				device_key,
				tamanu_version,
				state: RwLock::new(State::Tailscale(http)),
			}));
		}

		if let Some(pem) = device_key_pem {
			debug!("canopy: tailscale unreachable, falling back to mTLS");
			let http = build_mtls_http(pem)?;
			return Ok(Some(Self {
				device_key,
				tamanu_version,
				state: RwLock::new(State::Mtls(http)),
			}));
		}

		Ok(None)
	}

	/// Returns true if the client is currently using the tailscale path.
	pub async fn is_tailscale(&self) -> bool {
		self.state.read().await.is_tailscale()
	}

	/// Re-probe tailscale and swap modes if the picture has changed.
	///
	/// Intended to be called when the daemon receives a reload signal.
	pub async fn refresh(&self) -> Result<()> {
		if let Some(http) = probe_tailscale().await {
			let mut state = self.state.write().await;
			if !state.is_tailscale() {
				debug!("canopy refresh: switching to tailscale path");
			}
			*state = State::Tailscale(http);
			return Ok(());
		}

		if let Some(pem) = &self.device_key {
			let http = build_mtls_http(&pem.0)?;
			let mut state = self.state.write().await;
			if state.is_tailscale() {
				debug!("canopy refresh: tailscale dropped, falling back to mTLS");
			}
			*state = State::Mtls(http);
			return Ok(());
		}

		debug!("canopy refresh: no auth path available, keeping current state");
		Ok(())
	}

	/// Rebuild the underlying HTTP client with a fresh certificate.
	///
	/// No-op in tailscale mode (no cert to rotate). In mTLS mode, atomically
	/// replaces the live client; in-flight requests continue with the old
	/// client until they complete.
	pub async fn renew(&self) -> Result<()> {
		let Some(pem) = &self.device_key else {
			return Ok(());
		};
		let mut state = self.state.write().await;
		if state.is_tailscale() {
			return Ok(());
		}
		*state = State::Mtls(build_mtls_http(&pem.0)?);
		Ok(())
	}

	/// POST a status snapshot to the canopy server.
	///
	/// In tailscale mode, `base_url` is ignored and a `{TAILSCALE_URL}/public/status/{server_id}`
	/// URL is used. In mTLS mode, posts to `{base_url}/status/{server_id}`.
	///
	/// The payload is free-form JSON; the canopy `/status` contract reserves the
	/// top-level `healthy: bool` and `health: []` keys. The body is gzip-encoded
	/// with `Content-Encoding: gzip`.
	pub async fn post_status(
		&self,
		base_url: &Url,
		server_id: &str,
		payload: &serde_json::Value,
	) -> Result<()> {
		let (http, url) = {
			let state = self.state.read().await;
			let url = match &*state {
				State::Tailscale(_) => format!("{TAILSCALE_URL}/public/status/{server_id}")
					.parse::<Url>()
					.into_diagnostic()
					.wrap_err("building tailscale /public/status URL")?,
				State::Mtls(_) => base_url
					.join(&format!("/status/{server_id}"))
					.into_diagnostic()
					.wrap_err("building /status URL")?,
			};
			(state.http(), url)
		};

		let raw = serde_json::to_vec(payload)
			.into_diagnostic()
			.wrap_err("serialising canopy /status payload")?;
		let compressed = gzip_bytes(&raw)
			.into_diagnostic()
			.wrap_err("gzipping canopy /status payload")?;

		debug!(
			%url,
			raw_bytes = raw.len(),
			gzip_bytes = compressed.len(),
			"posting status snapshot to canopy",
		);

		let response = http
			.post(url)
			.header("X-Version", &self.tamanu_version)
			.header(reqwest::header::CONTENT_TYPE, "application/json")
			.header(reqwest::header::CONTENT_ENCODING, "gzip")
			.body(compressed)
			.send()
			.await
			.into_diagnostic()
			.wrap_err("posting status to canopy")?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			return Err(miette::miette!("canopy /status returned {status}: {body}"));
		}

		Ok(())
	}

	/// POST an event to the canopy server.
	///
	/// In tailscale mode, `base_url` is ignored and [`TAILSCALE_URL`] is used.
	/// In mTLS mode, posts to `{base_url}/events`.
	pub async fn post_event(&self, base_url: &Url, event: NewEvent<'_>) -> Result<()> {
		let (http, url) = {
			let state = self.state.read().await;
			let url = match &*state {
				State::Tailscale(_) => format!("{TAILSCALE_URL}/public/events")
					.parse::<Url>()
					.into_diagnostic()
					.wrap_err("building tailscale /public/events URL")?,
				State::Mtls(_) => base_url
					.join("/events")
					.into_diagnostic()
					.wrap_err("building /events URL")?,
			};
			(state.http(), url)
		};

		debug!(
			%url,
			source = event.source,
			r#ref = event.r#ref,
			active = ?event.active,
			"posting event to canopy"
		);

		let response = http
			.post(url)
			.header("X-Version", &self.tamanu_version)
			.json(&event)
			.send()
			.await
			.into_diagnostic()
			.wrap_err("posting event to canopy")?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			return Err(miette::miette!("canopy /events returned {status}: {body}"));
		}

		Ok(())
	}
}

/// Probe the tailscale canopy endpoint.
///
/// Returns a configured `reqwest::Client` if `GET /public/servers` responds
/// 2xx — anything else (timeout, non-2xx, transport error) returns `None` so
/// the caller can fall back to mTLS.
///
/// Tries two paths in order:
/// 1. Resolve `canopy` via the tailscale DNS server (100.100.100.100) and
///    probe with those addresses.
/// 2. Use hardcoded tailscale IPs for canopy and probe with those.
///
/// `/public/servers` is used because:
/// - it lives under `/public/...`, the only mount that accepts tagged-device
///   tailscale callers (everything else 403s with `tagged-device-not-allowed`);
/// - it's a `GET` with no body, no `VersionHeader` requirement, and no auth;
/// - it's read-only, so probing it has no side effects.
async fn probe_tailscale() -> Option<reqwest::Client> {
	let dns_addrs: Vec<SocketAddr> = tailscale_resolver()
		.lookup_ip("canopy")
		.await
		.ok()
		.map(|addrs| addrs.iter().map(|ip| SocketAddr::new(ip, 443)).collect())
		.unwrap_or_default();
	if !dns_addrs.is_empty()
		&& let Some(client) = try_probe(&dns_addrs).await
	{
		return Some(client);
	}

	let hardcoded = [
		SocketAddr::new(IpAddr::V4(CANOPY_HARDCODED_V4), 443),
		SocketAddr::new(IpAddr::V6(CANOPY_HARDCODED_V6), 443),
	];
	debug!(
		?hardcoded,
		"canopy tailscale DNS lookup empty or probe failed, trying hardcoded IPs"
	);
	try_probe(&hardcoded).await
}

async fn try_probe(addrs: &[SocketAddr]) -> Option<reqwest::Client> {
	let client = reqwest::Client::builder()
		.timeout(TAILSCALE_PROBE_TIMEOUT)
		.resolve_to_addrs(TAILSCALE_HOST, addrs)
		.build()
		.ok()?;

	let url = format!("{TAILSCALE_URL}/public/servers");
	match client.get(&url).send().await {
		Ok(resp) if resp.status().is_success() => Some(client),
		Ok(resp) => {
			debug!(status = %resp.status(), ?addrs, "canopy tailscale probe: unexpected status");
			None
		}
		Err(err) => {
			debug!(?addrs, "canopy tailscale probe failed: {err}");
			None
		}
	}
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

fn gzip_bytes(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
	let mut encoder = GzEncoder::new(Vec::with_capacity(bytes.len() / 2), Compression::default());
	encoder.write_all(bytes)?;
	encoder.finish()
}

fn build_mtls_http(device_key_pem: &str) -> Result<reqwest::Client> {
	let key_pair = KeyPair::from_pem(device_key_pem)
		.into_diagnostic()
		.wrap_err("parsing device key PEM")?;

	let mut params = CertificateParams::new(vec!["device.local".into()])
		.into_diagnostic()
		.wrap_err("building certificate params")?;
	params.distinguished_name = DistinguishedName::new();
	params
		.distinguished_name
		.push(DnType::CommonName, "device.local");

	let now = OffsetDateTime::now_utc();
	params.not_before = now - TimeDuration::minutes(1);
	params.not_after = now + TimeDuration::days(CERT_VALIDITY_DAYS);

	let cert = params
		.self_signed(&key_pair)
		.into_diagnostic()
		.wrap_err("self-signing certificate")?;

	let mut combined = cert.pem();
	combined.push('\n');
	combined.push_str(&key_pair.serialize_pem());

	let identity = reqwest::Identity::from_pem(combined.as_bytes())
		.into_diagnostic()
		.wrap_err("building reqwest TLS identity")?;

	reqwest::Client::builder()
		.identity(identity)
		.use_rustls_tls()
		.timeout(Duration::from_secs(30))
		.build()
		.into_diagnostic()
		.wrap_err("building canopy HTTP client")
}

#[cfg(test)]
mod tests {
	use super::*;

	const TEST_DEVICE_KEY: &str = "\
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgVvhzsYiidp38GYn1
KxD5Wipc/h8lglVsy1UFZq/SZbGhRANCAAT2EsEq7xjeWVnim9XwdYXga/LBbppm
fXLgamTYOa/w9n/Ta64fiYWmN54kEd0DgnflJDLtID321Zz6xswvK/VN
-----END PRIVATE KEY-----";

	#[test]
	fn build_mtls_http_from_p256_key() {
		// Direct mTLS-path build, bypassing the async constructor / tailscale probe.
		let result = build_mtls_http(TEST_DEVICE_KEY);
		assert!(result.is_ok(), "{:?}", result.err());
	}

	#[test]
	fn build_mtls_http_fails_on_garbage_key() {
		assert!(build_mtls_http("not a real PEM").is_err());
	}

	#[tokio::test]
	async fn renew_with_mtls_state_swaps_in_fresh_client() {
		// Construct an mTLS-state client directly (no network probe) and renew it.
		let http = build_mtls_http(TEST_DEVICE_KEY).unwrap();
		let client = CanopyClient {
			device_key: Some(Redacted(TEST_DEVICE_KEY.to_owned())),
			tamanu_version: "2.54.2".into(),
			state: RwLock::new(State::Mtls(http)),
		};
		client.renew().await.expect("renew should succeed");
		assert!(!client.is_tailscale().await);
	}

	#[tokio::test]
	async fn renew_is_noop_in_tailscale_mode() {
		// Tailscale-state client with no device key — renew is a no-op.
		let http = reqwest::Client::new();
		let client = CanopyClient {
			device_key: None,
			tamanu_version: "2.54.2".into(),
			state: RwLock::new(State::Tailscale(http)),
		};
		client.renew().await.expect("renew should be a no-op");
		assert!(client.is_tailscale().await);
	}

	#[test]
	fn gzip_bytes_roundtrips() {
		use flate2::read::GzDecoder;
		use std::io::Read;

		let original = br#"{"healthy":true,"health":[{"check":"x","healthy":true}]}"#;
		let compressed = gzip_bytes(original).expect("gzip should succeed");
		assert!(
			compressed.starts_with(&[0x1f, 0x8b]),
			"expected gzip magic bytes"
		);
		let mut decoder = GzDecoder::new(&compressed[..]);
		let mut decompressed = Vec::new();
		decoder.read_to_end(&mut decompressed).unwrap();
		assert_eq!(decompressed, original);
	}

	#[test]
	fn severity_serialises_lowercase() {
		assert_eq!(
			serde_json::to_string(&Severity::Warning).unwrap(),
			"\"warning\""
		);
		assert_eq!(
			serde_json::to_string(&Severity::Emergency).unwrap(),
			"\"emergency\""
		);
	}

	#[test]
	fn new_event_omits_optional_fields() {
		let evt = NewEvent {
			source: "src",
			r#ref: "host/alert:tgt",
			message: "msg",
			description: None,
			severity: None,
			occurred_at: None,
			active: None,
		};
		let json = serde_json::to_string(&evt).unwrap();
		assert!(json.contains("\"source\":\"src\""));
		assert!(json.contains("\"ref\":\"host/alert:tgt\""));
		assert!(json.contains("\"message\":\"msg\""));
		assert!(!json.contains("description"));
		assert!(!json.contains("severity"));
		assert!(!json.contains("occurredAt"));
		assert!(!json.contains("active"));
	}

	#[test]
	fn new_event_serialises_occurred_at_as_camel_case() {
		let evt = NewEvent {
			source: "src",
			r#ref: "ref",
			message: "msg",
			description: Some("desc"),
			severity: Some(Severity::Warning),
			occurred_at: Some("2025-01-01T00:00:00Z".parse().unwrap()),
			active: Some(true),
		};
		let json = serde_json::to_string(&evt).unwrap();
		assert!(json.contains("\"occurredAt\":"));
		assert!(json.contains("\"description\":\"desc\""));
		assert!(json.contains("\"severity\":\"warning\""));
		assert!(json.contains("\"active\":true"));
	}
}
