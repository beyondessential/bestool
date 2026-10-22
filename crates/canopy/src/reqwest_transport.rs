//! The default [`CanopyTransport`]: a [`reqwest`] client that picks canopy's
//! auth path (tailscale or mTLS) and routes calls accordingly.

use std::{
	fmt,
	future::Future,
	net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
	sync::{Arc, Mutex, OnceLock},
	time::{Duration, Instant},
};

use hickory_resolver::{
	ConnectionProvider, Resolver,
	config::{ConnectionConfig, NameServerConfig, ResolverConfig},
	net::runtime::TokioRuntimeProvider,
};
use miette::{IntoDiagnostic, Result, WrapErr, bail};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use reqwest::Url;
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::sync::RwLock;
use tracing::debug;

use bes_canopy_api::Error;

use crate::{CanopyRequest, CanopyResponse, CanopyTransport, Redacted, names::problem_slug_of};

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

/// Per-request timeout for a raw GET, overriding the tailscale probe client's
/// few seconds, since it covers downloading an artifact's body.
const RAW_GET_TIMEOUT: Duration = Duration::from_secs(300);

/// Timeout for the tailscale DNS lookup (against 100.100.100.100).
///
/// Bounds the lookup so a wedged tailscale DNS server can't stall discovery;
/// on timeout we fall back to the hardcoded IPs, which are probed concurrently
/// anyway.
const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a tailnet-reachability discovery is trusted before re-probing.
///
/// Short enough that tailscale coming up or going down is picked up promptly,
/// long enough that a burst of client constructions in one process shares a
/// single discovery instead of each paying the probe cost.
const PROBE_CACHE_TTL: Duration = Duration::from_secs(60);

/// Factory producing the base [`reqwest::ClientBuilder`] for canopy's clients.
///
/// The caller supplies this so it owns cross-cutting client config
/// (`SSLKEYLOGFILE`, proxies, …). Canopy invokes it whenever it needs to build or
/// rebuild a client — at probe time, on mTLS cert renewal, and on reload — then
/// layers its own concerns (its [`user_agent`], mTLS identity, DNS overrides,
/// timeouts, and its no-redirect policy) on top.
pub type ClientBuilderFactory = Arc<dyn Fn() -> reqwest::ClientBuilder + Send + Sync>;

/// User-agent set on every canopy request, e.g.
/// `bestool-canopy/0.5.0 (Linux 7.0.9 Arch Linux; x86_64)`.
///
/// Identifies this client crate and its version; the OS comment is detected at
/// runtime and cached. The transport sets this itself on top of the caller's
/// [`ClientBuilderFactory`], so canopy traffic identifies the client library
/// regardless of the calling binary.
fn user_agent() -> &'static str {
	static UA: OnceLock<String> = OnceLock::new();
	UA.get_or_init(|| {
		let os = sysinfo::System::long_os_version()
			.or_else(sysinfo::System::name)
			.unwrap_or_else(|| std::env::consts::OS.to_owned());
		format!(
			"bestool-canopy/{} ({os}; {})",
			env!("CARGO_PKG_VERSION"),
			sysinfo::System::cpu_arch(),
		)
	})
}

/// Probe the canopy tailnet endpoint, returning a client routed to it if
/// reachable.
///
/// The returned client carries the same DNS / hardcoded-IP resolution override
/// the reporting client uses and presents **no** client certificate — callers
/// reaching canopy this way authenticate by tailnet identity. Returns `None`
/// when the tailnet endpoint isn't reachable, so callers can fall back to
/// public mTLS.
pub async fn tailscale_client(make_builder: &ClientBuilderFactory) -> Option<reqwest::Client> {
	let tailscale_url = TAILSCALE_URL
		.parse()
		.expect("default tailscale URL is valid");
	probe_tailscale(&tailscale_url, make_builder, true).await
}

/// The default canopy transport: HTTP with auth configured for talking to a
/// canopy server.
///
/// Tries two auth paths in order of preference:
/// 1. **Tailscale**: if the canopy tailnet endpoint is reachable, plain HTTPS
///    works (auth is implicit via tailscale identity).
/// 2. **mTLS**: a fresh self-signed cert from the device key, short-lived
///    ([`CERT_VALIDITY_DAYS`]); for long-running daemons, [`Self::renew`]
///    should tick on [`CERT_RENEW_AFTER`] to swap in a fresh cert before expiry.
///
/// The tailscale probe asks an endpoint that needs no auth, so it cannot tell a
/// tailnet node canopy trusts from one it does not. When a call over tailscale is
/// refused for the tailnet identity (`auth-tailnet-node-not-permitted`) and a
/// device key is held, the transport falls back to mTLS and retries the call.
/// Generated calls, raw GETs and artifact downloads retry this way; a request
/// started with `request` is the caller's to send, so it gets the refusal.
///
/// [`Self::refresh`] re-probes tailscale and swaps modes on reload.
///
/// [`connect`](crate::connect) and [`connect_to`](crate::connect_to) build one
/// of these, so callers on the default transport never need to name it.
pub struct ReqwestTransport {
	/// Base URL for the mTLS path (canopy's public API, from the registration's
	/// `api_url`). Used only on the mTLS path. Fixed for the transport's lifetime.
	base_url: Url,
	/// Base URL for the tailscale path, or `None` to never use it (see
	/// [`tailnet_for`]). Fixed for the transport's lifetime.
	tailscale_url: Option<Url>,
	device_key: Option<Redacted<String>>,
	/// Produces the base client builder; see [`ClientBuilderFactory`].
	make_builder: ClientBuilderFactory,
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

impl fmt::Debug for ReqwestTransport {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("ReqwestTransport").finish_non_exhaustive()
	}
}

impl ReqwestTransport {
	/// Build a transport against explicit endpoints.
	///
	/// `base_url` is canopy's public API URL (the registration's `api_url`), used
	/// on the mTLS path; `tailscale_url` is the tailnet endpoint used on the
	/// tailscale path. Both are fixed for the transport's lifetime.
	///
	/// Probes the tailscale endpoint first; if reachable, uses it. Otherwise, if
	/// a device key PEM is provided, builds an mTLS client. Returns `Ok(None)` if
	/// neither path is available.
	///
	/// `make_builder` supplies the base [`reqwest::ClientBuilder`] — see
	/// [`ClientBuilderFactory`].
	pub async fn new(
		base_url: Url,
		tailscale_url: Option<Url>,
		device_key_pem: Option<&str>,
		make_builder: impl Fn() -> reqwest::ClientBuilder + Send + Sync + 'static,
	) -> Result<Option<Self>> {
		let device_key = device_key_pem.map(|s| Redacted(s.to_owned()));
		let make_builder: ClientBuilderFactory = Arc::new(make_builder);

		if let Some(http) = probe_optional(tailscale_url.as_ref(), &make_builder, true).await {
			debug!("canopy: tailscale endpoint reachable, preferring it");
			return Ok(Some(Self {
				base_url,
				tailscale_url,
				device_key,
				make_builder,
				state: RwLock::new(State::Tailscale(http)),
			}));
		}

		if let Some(pem) = device_key_pem {
			debug!("canopy: tailscale unreachable, falling back to mTLS");
			let http = build_mtls_http(&make_builder, pem)?;
			return Ok(Some(Self {
				base_url,
				tailscale_url,
				device_key,
				make_builder,
				state: RwLock::new(State::Mtls(http)),
			}));
		}

		Ok(None)
	}

	/// An mTLS-state transport against `base`, built without a network probe.
	#[cfg(test)]
	pub(crate) fn mtls_for_tests(base: &str) -> Self {
		use crate::test_support::{TEST_DEVICE_KEY, test_factory};

		let http = build_mtls_http(&test_factory(), TEST_DEVICE_KEY).unwrap();
		Self {
			base_url: base.parse().unwrap(),
			tailscale_url: Some(TAILSCALE_URL.parse().unwrap()),
			device_key: Some(Redacted(TEST_DEVICE_KEY.to_owned())),
			make_builder: test_factory(),
			state: RwLock::new(State::Mtls(http)),
		}
	}

	/// Returns true if the transport is currently using the tailscale path.
	pub async fn is_tailscale(&self) -> bool {
		self.state.read().await.is_tailscale()
	}

	/// Re-probe tailscale and swap modes if the picture has changed.
	///
	/// Intended to be called when the daemon receives a reload signal.
	pub async fn refresh(&self) -> Result<()> {
		if let Some(http) =
			probe_optional(self.tailscale_url.as_ref(), &self.make_builder, false).await
		{
			let mut state = self.state.write().await;
			if !state.is_tailscale() {
				debug!("canopy refresh: switching to tailscale path");
			}
			*state = State::Tailscale(http);
			return Ok(());
		}

		if let Some(pem) = &self.device_key {
			let http = build_mtls_http(&self.make_builder, &pem.0)?;
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
		*state = State::Mtls(build_mtls_http(&self.make_builder, &pem.0)?);
		Ok(())
	}

	/// Resolve the HTTP client + URL for `path` on the current auth path.
	///
	/// `path` is the mTLS-mode path (e.g. `/backup-target`); over tailscale the
	/// same endpoint is mounted under `/public`, so this prepends it.
	///
	/// The flag is whether that path is tailscale, so a caller whose request
	/// canopy refuses for its tailnet identity knows whether mTLS is left to try.
	///
	/// A join failure is reported as [`Error::transport`]: the request never
	/// reaches canopy, which is what that variant means.
	async fn endpoint_url(
		&self,
		path: &str,
	) -> bes_canopy_api::Result<(reqwest::Client, Url, bool)> {
		let state = self.state.read().await;
		let url = match &*state {
			State::Tailscale(_) => tailnet(&self.tailscale_url)
				.join(&format!("/public{path}"))
				.map_err(Error::transport)?,
			State::Mtls(_) => self.base_url.join(path).map_err(Error::transport)?,
		};
		Ok((state.http(), url, state.is_tailscale()))
	}

	/// Leave the tailscale path for mTLS after canopy refused the tailnet
	/// identity. Returns whether mTLS is now carrying calls, which needs a device
	/// key.
	///
	/// The probe that picks tailscale hits an unauthenticated endpoint, so it
	/// cannot tell a tailnet node canopy trusts from one it does not; only a
	/// real call learns that.
	async fn leave_tailnet(&self) -> Result<bool> {
		let Some(pem) = &self.device_key else {
			return Ok(false);
		};
		let mut state = self.state.write().await;
		if state.is_tailscale() {
			debug!("canopy refused the tailnet identity, falling back to mTLS");
			*state = State::Mtls(build_mtls_http(&self.make_builder, &pem.0)?);
		}
		Ok(true)
	}

	/// GET a path, routed via tailscale when available, returning the raw response.
	///
	/// Escape hatch behind the generated endpoint methods; needs the `raw-requests`
	/// feature. In tailscale mode the request goes to `{tailscale_url}{tailscale_path}`
	/// (typically `/public/...`); in mTLS mode to `{base_url}{mtls_path}`.
	///
	/// Redirects are not followed: a redirect, or an answer from any origin but
	/// the one addressed, is refused rather than returned.
	#[cfg(feature = "raw-requests")]
	pub async fn get(&self, tailscale_path: &str, mtls_path: &str) -> Result<reqwest::Response> {
		self.raw_get(tailscale_path, mtls_path).await
	}

	/// Download an artifact canopy offers. The device credential rides this
	/// request, so an offer naming any origin but canopy's is refused, and only
	/// its path and query are used.
	pub async fn download_artifact(&self, offered_url: &str) -> Result<reqwest::Response> {
		let path = self.own_path(offered_url)?;
		self.raw_get(&format!("/public{path}"), &path).await
	}

	/// Whether an offered URL is one canopy serves itself, as against a location
	/// it records for an artifact held elsewhere.
	pub fn holds(&self, offered_url: &str) -> bool {
		self.own_path(offered_url).is_ok()
	}

	/// The path and query to ask canopy for, from a URL canopy named.
	fn own_path(&self, offered_url: &str) -> Result<String> {
		let url = reqwest::Url::parse(offered_url)
			.into_diagnostic()
			.wrap_err("parsing the offered download URL")?;
		if url.origin() != self.base_url.origin() {
			bail!("the offered download URL {url} does not name canopy");
		}

		// A path opening with `//` names another host once joined to the base.
		if url.path().starts_with("//") {
			bail!("the offered download URL {url} carries an authority in its path");
		}

		Ok(match url.query() {
			Some(query) => format!("{}?{query}", url.path()),
			None => url.path().to_owned(),
		})
	}

	/// GET a path, routed via tailscale when available, returning the raw response.
	///
	/// Redirects are not followed: a redirect, or an answer from any origin but
	/// the one addressed, is refused rather than returned.
	async fn raw_get(&self, tailscale_path: &str, mtls_path: &str) -> Result<reqwest::Response> {
		let (response, via_tailnet) = self.raw_get_once(tailscale_path, mtls_path).await?;
		if !via_tailnet || response.status() != reqwest::StatusCode::FORBIDDEN {
			return Ok(response);
		}

		let status = response.status();
		let version = response.version();
		let headers = response.headers().clone();
		let body = response
			.bytes()
			.await
			.into_diagnostic()
			.wrap_err("reading canopy's refusal")?;
		if refuses_tailnet_identity(status, &body) && self.leave_tailnet().await? {
			return Ok(self.raw_get_once(tailscale_path, mtls_path).await?.0);
		}

		let mut refusal = http::Response::new(body);
		*refusal.status_mut() = status;
		*refusal.version_mut() = version;
		*refusal.headers_mut() = headers;
		Ok(reqwest::Response::from(refusal))
	}

	/// One attempt of [`Self::raw_get`] on the current auth path, reporting
	/// whether that path was tailscale.
	async fn raw_get_once(
		&self,
		tailscale_path: &str,
		mtls_path: &str,
	) -> Result<(reqwest::Response, bool)> {
		let (http, url, via_tailnet) = {
			let state = self.state.read().await;
			let url = match &*state {
				State::Tailscale(_) => tailnet(&self.tailscale_url)
					.join(tailscale_path)
					.into_diagnostic()
					.wrap_err("building tailscale GET URL")?,
				State::Mtls(_) => self
					.base_url
					.join(mtls_path)
					.into_diagnostic()
					.wrap_err("building mTLS GET URL")?,
			};
			(state.http(), url, state.is_tailscale())
		};

		debug!(%url, "GET via canopy");
		let response = http
			.get(url.clone())
			.timeout(RAW_GET_TIMEOUT)
			.send()
			.await
			.into_diagnostic()
			.wrap_err("GET via canopy")?;

		// The device credential rides this request, and alertd runs its body as DDL.
		if response.status().is_redirection() {
			bail!("canopy GET {url} was redirected, not answered");
		}
		if response.url().origin() != url.origin() {
			bail!("canopy GET {url} was answered from {}", response.url());
		}

		Ok((response, via_tailnet))
	}

	/// Start a request to an arbitrary canopy endpoint on the current auth path.
	///
	/// Escape hatch behind the generated endpoint methods; needs the `raw-requests`
	/// feature. `path` is the mTLS-mode path; over tailscale it's routed under
	/// `/public`, the same convention the generated methods follow.
	#[cfg(feature = "raw-requests")]
	pub async fn request(
		&self,
		method: reqwest::Method,
		path: &str,
	) -> Result<reqwest::RequestBuilder> {
		let (http, url, _) = self.endpoint_url(path).await.into_diagnostic()?;
		debug!(%url, %method, "arbitrary canopy request");
		Ok(http.request(method, url))
	}
}

#[async_trait::async_trait]
impl CanopyTransport for ReqwestTransport {
	async fn call(&self, request: CanopyRequest) -> bes_canopy_api::Result<CanopyResponse> {
		let (response, via_tailnet) = self.send(&request).await?;
		if via_tailnet
			&& refuses_tailnet_identity(response.status(), response.body())
			&& self
				.leave_tailnet()
				.await
				.map_err(|err| Error::Transport(err.into()))?
		{
			return Ok(self.send(&request).await?.0);
		}
		Ok(response)
	}
}

impl ReqwestTransport {
	/// One attempt of a generated call on the current auth path, reporting
	/// whether that path was tailscale.
	async fn send(
		&self,
		request: &CanopyRequest,
	) -> bes_canopy_api::Result<(CanopyResponse, bool)> {
		let path = request.uri().to_string();
		let (http, url, via_tailnet) = self.endpoint_url(&path).await?;
		debug!(%url, method = %request.method(), "canopy request");

		let mut req = http
			.request(request.method().clone(), url)
			.headers(request.headers().clone());
		if !request.body().is_empty() {
			req = req.body(request.body().clone());
		}

		let response = req.send().await.map_err(Error::transport)?;

		let status = response.status();
		let version = response.version();
		let headers = response.headers().clone();
		let body = response.bytes().await.map_err(Error::transport)?;

		let mut out = http::Response::new(body);
		*out.status_mut() = status;
		*out.version_mut() = version;
		*out.headers_mut() = headers;
		Ok((out, via_tailnet))
	}
}

/// The problem type canopy refuses a tailnet node it does not accept with.
const TAILNET_NODE_NOT_PERMITTED: &str = "auth-tailnet-node-not-permitted";

/// Whether canopy's answer says it does not accept the tailnet identity the
/// request came from, as against any other refusal, which mTLS would not change.
fn refuses_tailnet_identity(status: reqwest::StatusCode, body: &[u8]) -> bool {
	status == reqwest::StatusCode::FORBIDDEN
		&& problem_slug_of(body).as_deref() == Some(TAILNET_NODE_NOT_PERMITTED)
}

/// Probe the tailscale canopy endpoint, returning a configured `reqwest::Client`
/// routed to it if reachable and `None` otherwise (so callers fall back to mTLS).
///
/// For canopy's own tailnet endpoint the work is short-circuited and shared:
/// 1. **Gate** — if no tailscale interface is present on this host
///    ([`tailscale_present`]), the tailnet is unreachable by definition, so
///    skip all network I/O and return `None` immediately.
/// 2. **Cache** — when `use_cache` is set, a discovery from the last
///    [`PROBE_CACHE_TTL`] is reused instead of re-probing. `refresh` passes
///    `false` to force a fresh discovery on reload.
/// 3. **Discovery** — the tailscale-DNS-resolved probe and the hardcoded-IP
///    probe run *concurrently* ([`discover_tailnet`]); the first success wins.
///
/// `GET /public/servers` is the probe target because:
/// - it lives under `/public/...`, the only mount that accepts tagged-device
///   tailscale callers (everything else 403s with `tagged-device-not-allowed`);
/// - it's a `GET` with no body, no `VersionHeader` requirement, and no auth;
/// - it's read-only, so probing it has no side effects.
async fn probe_tailscale(
	tailscale_url: &Url,
	make_builder: &ClientBuilderFactory,
	use_cache: bool,
) -> Option<reqwest::Client> {
	let host = tailscale_url.host_str()?;

	// The gate, cache, and hardcoded-IP discovery below are specific to canopy's
	// own tailnet endpoint; probe any other tailscale URL with plain resolution.
	if host != TAILSCALE_HOST {
		return probe_once(tailscale_url, host, &[], make_builder).await;
	}

	if use_cache && let Some(outcome) = cached_outcome() {
		debug!("canopy: reusing cached tailnet reachability");
		return match outcome {
			TailnetOutcome::Unreachable => None,
			TailnetOutcome::Reachable(addrs) => build_probe_client(host, &addrs, make_builder),
		};
	}

	let discovered = discover_tailnet(tailscale_url, host, make_builder).await;
	store_outcome(match &discovered {
		Some((addrs, _)) => TailnetOutcome::Reachable(addrs.clone()),
		None => TailnetOutcome::Unreachable,
	});
	discovered.map(|(_, client)| client)
}

/// Discover a reachable route to the canopy tailnet endpoint, or `None`.
///
/// Returns the addresses that worked alongside the client built for them, so
/// the caller can both cache the route and reuse the client without rebuilding.
async fn discover_tailnet(
	tailscale_url: &Url,
	host: &str,
	make_builder: &ClientBuilderFactory,
) -> Option<(Vec<SocketAddr>, reqwest::Client)> {
	if !tailscale_present() {
		debug!("canopy: no tailscale interface on this host; skipping tailnet probe");
		return None;
	}

	let via_dns = async {
		let addrs = resolve_via_tailscale_dns().await;
		if addrs.is_empty() {
			return None;
		}
		probe_once(tailscale_url, host, &addrs, make_builder)
			.await
			.map(|client| (addrs, client))
	};

	let via_hardcoded = async {
		let addrs = vec![
			SocketAddr::new(IpAddr::V4(CANOPY_HARDCODED_V4), 443),
			SocketAddr::new(IpAddr::V6(CANOPY_HARDCODED_V6), 443),
		];
		probe_once(tailscale_url, host, &addrs, make_builder)
			.await
			.map(|client| (addrs, client))
	};

	race_first_some(via_dns, via_hardcoded).await
}

/// Resolve `canopy` via the tailscale DNS server (100.100.100.100), bounded by
/// [`DNS_LOOKUP_TIMEOUT`]. Returns an empty vec on timeout or lookup failure.
async fn resolve_via_tailscale_dns() -> Vec<SocketAddr> {
	match tokio::time::timeout(DNS_LOOKUP_TIMEOUT, tailscale_resolver().lookup_ip("canopy")).await {
		Ok(Ok(addrs)) => addrs.iter().map(|ip| SocketAddr::new(ip, 443)).collect(),
		Ok(Err(err)) => {
			debug!("canopy tailscale DNS lookup failed: {err}");
			Vec::new()
		}
		Err(_) => {
			debug!("canopy tailscale DNS lookup timed out");
			Vec::new()
		}
	}
}

/// Build the probe client for `host`, resolving it to `addrs` when non-empty
/// (the tailnet-discovery override); otherwise plain DNS is used.
fn build_probe_client(
	host: &str,
	addrs: &[SocketAddr],
	make_builder: &ClientBuilderFactory,
) -> Option<reqwest::Client> {
	let mut builder = make_builder()
		.user_agent(user_agent())
		.redirect(reqwest::redirect::Policy::none())
		.timeout(TAILSCALE_PROBE_TIMEOUT);
	if !addrs.is_empty() {
		builder = builder.resolve_to_addrs(host, addrs);
	}
	builder.build().ok()
}

/// Build a client for `addrs` and confirm `GET {tailscale_url}/public/servers`
/// responds 2xx; return the client on success, `None` on any other outcome.
async fn probe_once(
	tailscale_url: &Url,
	host: &str,
	addrs: &[SocketAddr],
	make_builder: &ClientBuilderFactory,
) -> Option<reqwest::Client> {
	let client = build_probe_client(host, addrs, make_builder)?;
	let url = tailscale_url.join("/public/servers").ok()?;
	match client.get(url).send().await {
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

/// Await two probes concurrently, resolving to the first that yields `Some`.
///
/// If the first to finish yields `None`, the other is awaited to completion.
async fn race_first_some<T>(
	a: impl Future<Output = Option<T>>,
	b: impl Future<Output = Option<T>>,
) -> Option<T> {
	use futures::future::{Either, select};

	let a = std::pin::pin!(a);
	let b = std::pin::pin!(b);
	match select(a, b).await {
		Either::Left((Some(v), _)) => Some(v),
		Either::Right((Some(v), _)) => Some(v),
		Either::Left((None, rest)) => rest.await,
		Either::Right((None, rest)) => rest.await,
	}
}

/// Whether any local interface holds a tailscale-assigned address.
///
/// Tailscale hands out IPv4 from the `100.64.0.0/10` CGNAT range and IPv6 from
/// its `fd7a:115c:a1e0::/48` ULA prefix. When neither is present the host isn't
/// on the tailnet, so probing canopy's tailnet endpoint can only ever time out
/// — the check lets us skip it and go straight to mTLS. A host that reaches the
/// tailnet purely through a subnet router (no address of its own) is treated as
/// absent and falls back to mTLS, which still works.
fn tailscale_present() -> bool {
	sysinfo::Networks::new_with_refreshed_list()
		.values()
		.flat_map(|net| net.ip_networks())
		.any(|net| is_tailscale_addr(&net.addr))
}

fn is_tailscale_addr(addr: &IpAddr) -> bool {
	match addr {
		IpAddr::V4(v4) => {
			let o = v4.octets();
			o[0] == 100 && (64..=127).contains(&o[1])
		}
		IpAddr::V6(v6) => {
			let s = v6.segments();
			s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
		}
	}
}

/// Outcome of a tailnet-reachability discovery, cached for [`PROBE_CACHE_TTL`].
#[derive(Clone)]
enum TailnetOutcome {
	/// Reachable via these addresses (empty = plain DNS resolution worked).
	Reachable(Vec<SocketAddr>),
	Unreachable,
}

struct CachedProbe {
	stored_at: Instant,
	outcome: TailnetOutcome,
}

fn probe_cache() -> &'static Mutex<Option<CachedProbe>> {
	static CACHE: OnceLock<Mutex<Option<CachedProbe>>> = OnceLock::new();
	CACHE.get_or_init(|| Mutex::new(None))
}

/// The cached outcome if one was stored within the last [`PROBE_CACHE_TTL`].
fn cached_outcome() -> Option<TailnetOutcome> {
	let guard = probe_cache().lock().expect("canopy probe cache poisoned");
	let entry = guard.as_ref()?;
	(entry.stored_at.elapsed() < PROBE_CACHE_TTL).then(|| entry.outcome.clone())
}

fn store_outcome(outcome: TailnetOutcome) {
	*probe_cache().lock().expect("canopy probe cache poisoned") = Some(CachedProbe {
		stored_at: Instant::now(),
		outcome,
	});
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

/// Build a short-lived self-signed client certificate from a P-256 device key
/// PEM and wrap it as a reqwest mTLS [`Identity`].
///
/// Canopy identifies a device by its certificate's public key (SPKI), not by a
/// CA chain, so a fresh self-signed cert from the device key is all that's
/// needed. The same device key drives both the long-running canopy client here
/// and the one-shot `canopy register` enrollment handshake, so they present the
/// same identity to canopy.
///
/// [`Identity`]: reqwest::Identity
pub fn device_identity(device_key_pem: &str) -> Result<reqwest::Identity> {
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

	reqwest::Identity::from_pem(combined.as_bytes())
		.into_diagnostic()
		.wrap_err("building reqwest TLS identity")
}

fn build_mtls_http(
	make_builder: &ClientBuilderFactory,
	device_key_pem: &str,
) -> Result<reqwest::Client> {
	let identity = device_identity(device_key_pem)?;

	make_builder()
		.user_agent(user_agent())
		.identity(identity)
		.redirect(reqwest::redirect::Policy::none())
		.use_rustls_tls()
		.timeout(Duration::from_secs(30))
		.build()
		.into_diagnostic()
		.wrap_err("building canopy HTTP client")
}

/// The tailnet endpoint to try for a device registered against `base_url`.
///
/// The tailnet route only reaches the default canopy, so a device registered
/// with any other canopy must not take it.
pub fn tailnet_for(base_url: &Url) -> Option<Url> {
	let default: Url = DEFAULT_CANOPY_URL
		.parse()
		.expect("default canopy URL is valid");
	(base_url.origin() == default.origin()).then(|| {
		TAILSCALE_URL
			.parse()
			.expect("default tailscale URL is valid")
	})
}

fn tailnet(tailscale_url: &Option<Url>) -> &Url {
	tailscale_url
		.as_ref()
		.expect("the tailscale state is only entered after probing a tailnet endpoint")
}

async fn probe_optional(
	tailscale_url: Option<&Url>,
	make_builder: &ClientBuilderFactory,
	discover: bool,
) -> Option<reqwest::Client> {
	probe_tailscale(tailscale_url?, make_builder, discover).await
}

#[cfg(test)]
mod tests {
	use crate::test_support::{TEST_DEVICE_KEY, closed_url, serve_once, test_factory};

	use super::*;

	#[test]
	fn only_the_default_canopy_takes_the_tailnet() {
		assert_eq!(
			tailnet_for(&DEFAULT_CANOPY_URL.parse().unwrap()),
			Some(TAILSCALE_URL.parse().unwrap()),
		);
		assert_eq!(
			tailnet_for(&"https://canopy-local.test:8443".parse().unwrap()),
			None
		);
	}

	#[tokio::test]
	async fn transport_sets_the_canopy_user_agent_on_the_wire() {
		// A generated call over the real transport: it resolves the mTLS path and
		// stamps the canopy user-agent, whatever binary is calling.
		let (base, handle) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]");
		let client = crate::CanopyClient::new(ReqwestTransport::mtls_for_tests(&base));
		let servers = client.servers().await.expect("GET /servers should parse");
		assert!(servers.is_empty());

		let captured = handle.join().unwrap();
		assert!(
			captured.request_line.starts_with("GET /servers "),
			"unexpected request line: {}",
			captured.request_line
		);
		assert!(
			captured
				.headers
				.to_ascii_lowercase()
				.contains("user-agent: bestool-canopy/"),
			"missing canopy user-agent in:\n{}",
			captured.headers
		);
	}

	#[test]
	fn build_mtls_http_from_p256_key() {
		// Direct mTLS-path build, bypassing the async constructor / tailscale probe.
		let result = build_mtls_http(&test_factory(), TEST_DEVICE_KEY);
		assert!(result.is_ok(), "{:?}", result.err());
	}

	#[test]
	fn build_mtls_http_fails_on_garbage_key() {
		assert!(build_mtls_http(&test_factory(), "not a real PEM").is_err());
	}

	#[tokio::test]
	async fn no_device_key_still_builds_over_tailscale() {
		let (tailnet, _server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]");
		let transport = ReqwestTransport::new(
			DEFAULT_CANOPY_URL.parse().unwrap(),
			Some(tailnet.parse().unwrap()),
			None,
			reqwest::Client::builder,
		)
		.await
		.expect("keyless build should not error")
		.expect("a reachable tailnet is an auth path in its own right");
		assert!(transport.is_tailscale().await);
	}

	#[tokio::test]
	async fn no_device_key_and_no_tailnet_leaves_no_auth_path() {
		let transport = ReqwestTransport::new(
			DEFAULT_CANOPY_URL.parse().unwrap(),
			Some(closed_url().parse().unwrap()),
			None,
			reqwest::Client::builder,
		)
		.await
		.expect("keyless build should not error");
		assert!(transport.is_none());
	}

	#[tokio::test]
	async fn device_key_carries_the_call_when_the_tailnet_is_unreachable() {
		let transport = ReqwestTransport::new(
			DEFAULT_CANOPY_URL.parse().unwrap(),
			Some(closed_url().parse().unwrap()),
			Some(TEST_DEVICE_KEY),
			reqwest::Client::builder,
		)
		.await
		.expect("mTLS build should not error")
		.expect("a device key is an auth path when the tailnet is out of reach");
		assert!(!transport.is_tailscale().await);
	}

	#[tokio::test]
	async fn renew_with_mtls_state_swaps_in_fresh_client() {
		let transport = ReqwestTransport::mtls_for_tests(DEFAULT_CANOPY_URL);
		transport.renew().await.expect("renew should succeed");
		assert!(!transport.is_tailscale().await);
	}

	#[tokio::test]
	async fn renew_is_noop_in_tailscale_mode() {
		// Tailscale-state transport with no device key — renew is a no-op.
		let transport = ReqwestTransport {
			base_url: DEFAULT_CANOPY_URL.parse().unwrap(),
			tailscale_url: Some(TAILSCALE_URL.parse().unwrap()),
			device_key: None,
			make_builder: test_factory(),
			state: RwLock::new(State::Tailscale(reqwest::Client::new())),
		};
		transport.renew().await.expect("renew should be a no-op");
		assert!(transport.is_tailscale().await);
	}

	/// The offer names where to fetch a schema and the device credential rides
	/// the request, so an offer naming any other origin is refused before it is
	/// followed.
	#[tokio::test]
	async fn a_download_url_off_canopy_is_refused() {
		let transport = ReqwestTransport::mtls_for_tests(DEFAULT_CANOPY_URL);
		let err = transport
			.download_artifact("https://evil.example/versions/2.60.0/artifacts/a/download")
			.await
			.expect_err("an offer off canopy is not followed");
		assert!(err.to_string().contains("does not name canopy"), "{err}");
	}

	/// Canopy records where an artifact it does not hold rests and hands that
	/// location back as the offer, so what says canopy will serve the bytes
	/// itself is the offer naming canopy.
	#[test]
	fn an_offer_resting_elsewhere_is_not_one_canopy_holds() {
		let transport = ReqwestTransport::mtls_for_tests(DEFAULT_CANOPY_URL);

		assert!(transport.holds(&format!(
			"{DEFAULT_CANOPY_URL}/versions/2.60.0/artifacts/a/download"
		)));

		for offered in [
			"https://releases.example/2.60.0/schema.sql",
			&format!("{DEFAULT_CANOPY_URL}//releases.example/schema.sql"),
			"not a URL",
		] {
			assert!(!transport.holds(offered), "{offered}");
		}
	}

	/// A path opening with `//` is an authority of its own, so it names canopy
	/// to the origin check and another host once resolved against the base.
	#[tokio::test]
	async fn a_download_url_with_an_authority_in_its_path_is_refused() {
		let transport = ReqwestTransport::mtls_for_tests(DEFAULT_CANOPY_URL);
		let err = transport
			.download_artifact(&format!("{DEFAULT_CANOPY_URL}//evil.example/x.sql"))
			.await
			.expect_err("an authority smuggled into the path is not followed");
		assert!(
			err.to_string().contains("carries an authority in its path"),
			"{err}"
		);
	}

	/// Canopy names its own origin in the offer, but which endpoint holds the
	/// credential is the transport's to decide, so the offer is followed for
	/// its path.
	#[tokio::test]
	async fn a_download_url_on_canopy_is_followed_by_path() {
		let (canopy, _server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nSELECT 1;");
		let transport = ReqwestTransport {
			base_url: DEFAULT_CANOPY_URL.parse().unwrap(),
			tailscale_url: Some(canopy.parse().unwrap()),
			device_key: None,
			make_builder: test_factory(),
			state: RwLock::new(State::Tailscale(
				build_probe_client("127.0.0.1", &[], &test_factory()).expect("a canopy client"),
			)),
		};

		let response = transport
			.download_artifact(&format!(
				"{DEFAULT_CANOPY_URL}/versions/2.60.0/artifacts/a/download"
			))
			.await
			.expect("canopy's own offer is followed");
		assert_eq!(
			response.url().path(),
			"/public/versions/2.60.0/artifacts/a/download"
		);
	}

	/// Canopy addresses the artifact, and a parameter it puts on the offer is
	/// part of that address: dropped, the fetch asks for something else.
	#[tokio::test]
	async fn a_download_url_keeps_the_query_canopy_named() {
		let (canopy, _server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nSELECT 1;");
		let transport = ReqwestTransport {
			base_url: DEFAULT_CANOPY_URL.parse().unwrap(),
			tailscale_url: Some(canopy.parse().unwrap()),
			device_key: None,
			make_builder: test_factory(),
			state: RwLock::new(State::Tailscale(
				build_probe_client("127.0.0.1", &[], &test_factory()).expect("a canopy client"),
			)),
		};

		let response = transport
			.download_artifact(&format!(
				"{DEFAULT_CANOPY_URL}/versions/2.60.0/artifacts/a/download?platform=linux"
			))
			.await
			.expect("canopy's own offer is followed");
		assert_eq!(response.url().query(), Some("platform=linux"));
	}

	/// A raw GET's body reaches alertd as privileged DDL and the device
	/// credential rides the request, so a hop off canopy's origin is refused.
	#[cfg(feature = "raw-requests")]
	#[tokio::test]
	async fn a_redirect_off_canopy_is_refused() {
		let (canopy, _canopy_server) = serve_once(
			"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/x\r\nContent-Length: 0\r\n\r\n",
		);

		let transport = ReqwestTransport {
			base_url: DEFAULT_CANOPY_URL.parse().unwrap(),
			tailscale_url: Some(canopy.parse().unwrap()),
			device_key: None,
			make_builder: test_factory(),
			state: RwLock::new(State::Tailscale(
				build_probe_client("127.0.0.1", &[], &test_factory()).expect("a canopy client"),
			)),
		};

		let err = transport
			.get("/public/x", "/x")
			.await
			.expect_err("a redirect is not canopy's answer");
		assert!(err.to_string().contains("redirected"), "{err}");
	}

	#[cfg(feature = "raw-requests")]
	#[tokio::test]
	async fn a_get_canopy_answers_itself_comes_back() {
		let (canopy, _server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nSELECT 1;");
		let transport = ReqwestTransport {
			base_url: DEFAULT_CANOPY_URL.parse().unwrap(),
			tailscale_url: Some(canopy.parse().unwrap()),
			device_key: None,
			make_builder: test_factory(),
			state: RwLock::new(State::Tailscale(
				build_probe_client("127.0.0.1", &[], &test_factory()).expect("a canopy client"),
			)),
		};

		let response = transport
			.get("/public/x", "/x")
			.await
			.expect("canopy answered");
		assert_eq!(response.text().await.unwrap(), "SELECT 1;");
	}

	/// A transport on the tailscale path against `tailnet`, with mTLS calls
	/// going to `canopy`.
	fn tailnet_transport(
		canopy: &str,
		tailnet: &str,
		device_key: Option<&str>,
	) -> ReqwestTransport {
		ReqwestTransport {
			base_url: canopy.parse().unwrap(),
			tailscale_url: Some(tailnet.parse().unwrap()),
			device_key: device_key.map(|key| Redacted(key.to_owned())),
			make_builder: test_factory(),
			state: RwLock::new(State::Tailscale(
				build_probe_client("127.0.0.1", &[], &test_factory()).expect("a tailnet client"),
			)),
		}
	}

	/// The tailscale probe asks an unauthenticated endpoint, so it passes for a
	/// tailnet node canopy does not accept; the refusal that follows on a real
	/// call is what moves the transport to mTLS.
	#[tokio::test]
	async fn a_refused_tailnet_identity_falls_back_to_mtls() {
		let (tailnet, _tailnet_server) = serve_once(
			"HTTP/1.1 403 Forbidden\r\nContent-Type: application/problem+json\r\nContent-Length: 63\r\n\r\n{\"type\":\"/errors/auth-tailnet-node-not-permitted\",\"title\":\"no\"}",
		);
		let (canopy, canopy_server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]");
		let client =
			crate::CanopyClient::new(tailnet_transport(&canopy, &tailnet, Some(TEST_DEVICE_KEY)));

		let servers = client
			.servers()
			.await
			.expect("the call is retried over mTLS");
		assert!(servers.is_empty());
		assert!(!client.transport().is_tailscale().await);

		let captured = canopy_server.join().unwrap();
		assert!(
			captured.request_line.starts_with("GET /servers "),
			"unexpected request line: {}",
			captured.request_line
		);
	}

	#[tokio::test]
	async fn a_refused_tailnet_identity_without_a_device_key_is_reported() {
		let (tailnet, _tailnet_server) = serve_once(
			"HTTP/1.1 403 Forbidden\r\nContent-Type: application/problem+json\r\nContent-Length: 63\r\n\r\n{\"type\":\"/errors/auth-tailnet-node-not-permitted\",\"title\":\"no\"}",
		);
		let client =
			crate::CanopyClient::new(tailnet_transport(DEFAULT_CANOPY_URL, &tailnet, None));

		let err = client
			.servers()
			.await
			.expect_err("with no device key there is nothing to fall back to");
		assert_eq!(err.http().map(|err| err.status.as_u16()), Some(403));
		assert!(client.transport().is_tailscale().await);
	}

	/// mTLS would not change any other refusal, so it is not tried.
	#[tokio::test]
	async fn another_refusal_does_not_leave_the_tailnet() {
		let (tailnet, _tailnet_server) = serve_once(
			"HTTP/1.1 403 Forbidden\r\nContent-Type: application/problem+json\r\nContent-Length: 61\r\n\r\n{\"type\":\"/errors/auth-insufficient-permissions\",\"title\":\"no\"}",
		);
		let client = crate::CanopyClient::new(tailnet_transport(
			&closed_url(),
			&tailnet,
			Some(TEST_DEVICE_KEY),
		));

		let err = client
			.servers()
			.await
			.expect_err("the refusal is canopy's answer");
		assert_eq!(err.http().map(|err| err.status.as_u16()), Some(403));
		assert!(client.transport().is_tailscale().await);
	}

	#[cfg(feature = "raw-requests")]
	#[tokio::test]
	async fn a_raw_get_refused_on_the_tailnet_falls_back_to_mtls() {
		let (tailnet, _tailnet_server) = serve_once(
			"HTTP/1.1 403 Forbidden\r\nContent-Type: application/problem+json\r\nContent-Length: 63\r\n\r\n{\"type\":\"/errors/auth-tailnet-node-not-permitted\",\"title\":\"no\"}",
		);
		let (canopy, _canopy_server) =
			serve_once("HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nSELECT 1;");
		let transport = tailnet_transport(&canopy, &tailnet, Some(TEST_DEVICE_KEY));

		let response = transport
			.get("/public/x", "/x")
			.await
			.expect("the GET is retried over mTLS");
		assert_eq!(response.text().await.unwrap(), "SELECT 1;");
		assert!(!transport.is_tailscale().await);
	}

	#[tokio::test]
	async fn tailscale_state_routes_under_public() {
		let transport = ReqwestTransport {
			base_url: DEFAULT_CANOPY_URL.parse().unwrap(),
			tailscale_url: Some("https://tailnet.example".parse().unwrap()),
			device_key: None,
			make_builder: test_factory(),
			state: RwLock::new(State::Tailscale(reqwest::Client::new())),
		};
		let (_, url, _) = transport.endpoint_url("/backup-target").await.unwrap();
		assert_eq!(url.as_str(), "https://tailnet.example/public/backup-target");
	}

	#[test]
	fn user_agent_identifies_the_crate_with_os_comment() {
		let ua = user_agent();
		assert!(
			ua.starts_with(concat!("bestool-canopy/", env!("CARGO_PKG_VERSION"), " ")),
			"unexpected user-agent: {ua}"
		);
		assert!(ua.contains('('), "expected OS comment in: {ua}");
		assert!(ua.ends_with(')'), "expected OS comment in: {ua}");
		assert!(
			ua.contains(sysinfo::System::cpu_arch().as_str()),
			"expected arch in: {ua}"
		);
	}

	#[test]
	fn tailscale_addr_classifies_cgnat_v4() {
		assert!(is_tailscale_addr(&IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
		assert!(is_tailscale_addr(&IpAddr::V4(Ipv4Addr::new(
			100, 127, 255, 255
		))));
		assert!(is_tailscale_addr(&IpAddr::V4(CANOPY_HARDCODED_V4)));
		// Just outside the 100.64.0.0/10 range on either side.
		assert!(!is_tailscale_addr(&IpAddr::V4(Ipv4Addr::new(
			100, 63, 255, 255
		))));
		assert!(!is_tailscale_addr(&IpAddr::V4(Ipv4Addr::new(
			100, 128, 0, 0
		))));
		// A plain public/private v4 must not read as tailscale.
		assert!(!is_tailscale_addr(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
		assert!(!is_tailscale_addr(&IpAddr::V4(Ipv4Addr::new(100, 0, 0, 1))));
	}

	#[test]
	fn tailscale_addr_classifies_ula_v6() {
		assert!(is_tailscale_addr(&IpAddr::V6(CANOPY_HARDCODED_V6)));
		assert!(is_tailscale_addr(&IpAddr::V6(Ipv6Addr::new(
			0xfd7a, 0x115c, 0xa1e0, 0, 0, 0, 0, 1
		))));
		// Different ULA prefix — not tailscale.
		assert!(!is_tailscale_addr(&IpAddr::V6(Ipv6Addr::new(
			0xfd00, 0x115c, 0xa1e0, 0, 0, 0, 0, 1
		))));
		assert!(!is_tailscale_addr(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
	}

	#[test]
	fn probe_cache_roundtrips_and_expires() {
		store_outcome(TailnetOutcome::Reachable(vec![SocketAddr::new(
			IpAddr::V4(CANOPY_HARDCODED_V4),
			443,
		)]));
		match cached_outcome() {
			Some(TailnetOutcome::Reachable(addrs)) => {
				assert_eq!(
					addrs,
					vec![SocketAddr::new(IpAddr::V4(CANOPY_HARDCODED_V4), 443)]
				);
			}
			other => panic!(
				"expected freshly stored Reachable, got {:?}",
				other.is_some()
			),
		}

		// A stale entry (stored before the TTL window) reads as a miss.
		// Guard the subtraction: a freshly started process may not have enough
		// monotonic headroom to represent an instant a full TTL in the past.
		if let Some(stale) = Instant::now().checked_sub(PROBE_CACHE_TTL + Duration::from_secs(1)) {
			*probe_cache().lock().unwrap() = Some(CachedProbe {
				stored_at: stale,
				outcome: TailnetOutcome::Unreachable,
			});
			assert!(cached_outcome().is_none());
		}
	}
}
