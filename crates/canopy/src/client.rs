use std::{
	fmt,
	io::Write,
	net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
	sync::{Arc, OnceLock},
	time::Duration,
};

use flate2::{Compression, write::GzEncoder};
use hickory_resolver::{
	ConnectionProvider, Resolver,
	config::{ConnectionConfig, NameServerConfig, ResolverConfig},
	net::runtime::TokioRuntimeProvider,
};
use miette::{IntoDiagnostic, Result, WrapErr};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use reqwest::Url;
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::sync::RwLock;
use tracing::debug;

use crate::{
	Redacted,
	backup::TargetOutcome,
	schema::{
		BackupPurpose, BackupTarget, CapabilitiesArgs, CredentialProcessOutput, CredentialsArgs,
		ReportArgs,
	},
};

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

/// Factory producing the base [`reqwest::ClientBuilder`] for canopy's clients.
///
/// The caller supplies this so it owns cross-cutting client config
/// (`SSLKEYLOGFILE`, proxies, …). Canopy invokes it whenever it needs to build or
/// rebuild a client — at probe time, on mTLS cert renewal, and on reload — then
/// layers its own concerns (its [`user_agent`], mTLS identity, DNS overrides,
/// timeouts) on top.
pub type ClientBuilderFactory = Arc<dyn Fn() -> reqwest::ClientBuilder + Send + Sync>;

/// User-agent set on every canopy request, e.g.
/// `bestool-canopy/0.5.0 (Linux 7.0.9 Arch Linux; x86_64)`.
///
/// Identifies this client crate and its version; the OS comment is detected at
/// runtime and cached. The client sets this itself on top of the caller's
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
	probe_tailscale(&tailscale_url, make_builder).await
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
	/// Base URL for the mTLS path (canopy's public API, from the registration's
	/// `api_url`). Used only on the mTLS path. Fixed for the client's lifetime.
	base_url: Url,
	/// Base URL for the tailscale path (defaults to [`TAILSCALE_URL`]). Used only
	/// on the tailscale path. Fixed for the client's lifetime.
	tailscale_url: Url,
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

impl fmt::Debug for CanopyClient {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CanopyClient").finish_non_exhaustive()
	}
}

impl CanopyClient {
	/// Build a canopy client against the default public ([`DEFAULT_CANOPY_URL`])
	/// and tailscale ([`TAILSCALE_URL`]) endpoints. Use [`Self::with_urls`] to
	/// override them.
	///
	/// Probes the tailscale endpoint first; if reachable, uses it. Otherwise, if
	/// a device key PEM is provided, builds an mTLS client. Returns `Ok(None)` if
	/// neither path is available.
	///
	/// `make_builder` supplies the base [`reqwest::ClientBuilder`] — see
	/// [`ClientBuilderFactory`].
	pub async fn new(
		device_key_pem: Option<&str>,
		make_builder: impl Fn() -> reqwest::ClientBuilder + Send + Sync + 'static,
	) -> Result<Option<Self>> {
		Self::with_urls(
			DEFAULT_CANOPY_URL
				.parse()
				.expect("default canopy URL is valid"),
			TAILSCALE_URL
				.parse()
				.expect("default tailscale URL is valid"),
			device_key_pem,
			make_builder,
		)
		.await
	}

	/// Build a canopy client against explicit endpoints.
	///
	/// `base_url` is canopy's public API URL (the registration's `api_url`),
	/// used on the mTLS path; `tailscale_url` is the tailnet endpoint used on
	/// the tailscale path. Both are fixed for the client's lifetime. See
	/// [`Self::new`] for the other arguments and the default-endpoint form.
	pub async fn with_urls(
		base_url: Url,
		tailscale_url: Url,
		device_key_pem: Option<&str>,
		make_builder: impl Fn() -> reqwest::ClientBuilder + Send + Sync + 'static,
	) -> Result<Option<Self>> {
		let device_key = device_key_pem.map(|s| Redacted(s.to_owned()));
		let make_builder: ClientBuilderFactory = Arc::new(make_builder);

		if let Some(http) = probe_tailscale(&tailscale_url, &make_builder).await {
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

	/// Returns true if the client is currently using the tailscale path.
	pub async fn is_tailscale(&self) -> bool {
		self.state.read().await.is_tailscale()
	}

	/// Re-probe tailscale and swap modes if the picture has changed.
	///
	/// Intended to be called when the daemon receives a reload signal.
	pub async fn refresh(&self) -> Result<()> {
		if let Some(http) = probe_tailscale(&self.tailscale_url, &self.make_builder).await {
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

	/// POST a status snapshot to the canopy server.
	///
	/// In tailscale mode a `{TAILSCALE_URL}/public/status/{server_id}` URL is
	/// used; in mTLS mode it posts to `{base_url}/status/{server_id}` for the
	/// client's configured base URL.
	///
	/// The payload is free-form JSON; the canopy `/status` contract reserves the
	/// top-level `health: []` key, whose entries each carry a `result` of
	/// `passed | warning | failed | broken | skipped`. The body is gzip-encoded
	/// with `Content-Encoding: gzip`.
	///
	/// Returns `backup_now`: the backup-type names canopy says this server should
	/// back up right now (operator one-offs + schedule-due). Empty means nothing
	/// to do. A response that predates the field (no `backup_now`) yields an empty
	/// list, so older canopy deployments keep working.
	pub async fn post_status(
		&self,
		server_id: &str,
		payload: &serde_json::Value,
	) -> Result<Vec<String>> {
		let (http, url) = {
			let state = self.state.read().await;
			let url = match &*state {
				State::Tailscale(_) => self
					.tailscale_url
					.join(&format!("/public/status/{server_id}"))
					.into_diagnostic()
					.wrap_err("building tailscale /public/status URL")?,
				State::Mtls(_) => self
					.base_url
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

		#[derive(serde::Deserialize, Default)]
		struct StatusResponseTail {
			#[serde(default)]
			backup_now: Vec<String>,
		}

		// The response flattens the persisted Status plus `backup_now`; we read
		// only the latter and ignore the rest. A body that fails to parse (or
		// predates the field) is treated as "nothing to do" rather than failing
		// the status push.
		let tail = response
			.json::<StatusResponseTail>()
			.await
			.unwrap_or_default();
		Ok(tail.backup_now)
	}

	/// GET a path on the canopy server, routed via tailscale when available.
	///
	/// In tailscale mode, the request goes to `{TAILSCALE_URL}{tailscale_path}`
	/// (typically `/public/...`, the only mount that accepts tagged-device
	/// tailscale callers). In mTLS mode, the request goes to `{base_url}{mtls_path}`
	/// for the client's configured base URL.
	///
	/// Returns the raw response — the caller is responsible for status checks
	/// and body parsing so they can choose how to fall back if the response
	/// isn't usable.
	pub async fn get(&self, tailscale_path: &str, mtls_path: &str) -> Result<reqwest::Response> {
		let (http, url) = {
			let state = self.state.read().await;
			let url = match &*state {
				State::Tailscale(_) => self
					.tailscale_url
					.join(tailscale_path)
					.into_diagnostic()
					.wrap_err("building tailscale GET URL")?,
				State::Mtls(_) => self
					.base_url
					.join(mtls_path)
					.into_diagnostic()
					.wrap_err("building mTLS GET URL")?,
			};
			(state.http(), url)
		};

		debug!(%url, "GET via canopy");
		http.get(url)
			.send()
			.await
			.into_diagnostic()
			.wrap_err("GET via canopy")
	}

	/// Resolve an endpoint URL for the current auth path.
	///
	/// `path` is the mTLS-mode path (e.g. `/backup-target`); over tailscale the
	/// same endpoint is mounted under `/public`, so this prepends it.
	async fn endpoint_url(&self, path: &str) -> Result<(reqwest::Client, Url)> {
		let state = self.state.read().await;
		let url = match &*state {
			State::Tailscale(_) => self
				.tailscale_url
				.join(&format!("/public{path}"))
				.into_diagnostic()
				.wrap_err_with(|| format!("building tailscale /public{path} URL"))?,
			State::Mtls(_) => self
				.base_url
				.join(path)
				.into_diagnostic()
				.wrap_err_with(|| format!("building {path} URL"))?,
		};
		Ok((state.http(), url))
	}

	/// Start a request to an arbitrary canopy endpoint on the current auth path.
	///
	/// This is the generic escape hatch behind the typed endpoint methods: it
	/// resolves the right HTTP client and URL for the active auth mode and begins
	/// the request. The returned builder is yours to finish — add query params, a
	/// body, or extra headers, then `.send()` and parse the response however suits.
	///
	/// `path` is the mTLS-mode path (e.g. `/backup-target`); over tailscale the
	/// same endpoint is mounted under `/public`, so this routes it there, the
	/// same convention the other endpoint methods follow.
	pub async fn request(
		&self,
		method: reqwest::Method,
		path: &str,
	) -> Result<reqwest::RequestBuilder> {
		let (http, url) = self.endpoint_url(path).await?;
		debug!(%url, %method, "arbitrary canopy request");
		Ok(http.request(method, url))
	}

	/// Call an arbitrary canopy endpoint and parse its JSON response.
	///
	/// Builds the request via [`Self::request`], attaches `body` as JSON when
	/// it's `Some`, sends it, and on a 2xx response parses the body into `Res`.
	/// A non-success status becomes an error carrying the status and response
	/// body, matching the other endpoint methods. This absorbs the status-check
	/// and parse boilerplate.
	///
	/// Use [`serde_json::Value`] for `Res` (and/or the body) for fully dynamic
	/// calls, or any concrete type for typed calls. When passing no body, pin
	/// the inference with a turbofish, e.g. `None::<&()>`.
	///
	/// `path` follows the same mTLS/tailscale convention as [`Self::request`].
	pub async fn request_json<Res: serde::de::DeserializeOwned>(
		&self,
		method: reqwest::Method,
		path: &str,
		body: Option<&(impl serde::Serialize + ?Sized)>,
	) -> Result<Res> {
		let mut req = self.request(method, path).await?;
		if let Some(body) = body {
			req = req.json(body);
		}

		let response = req
			.send()
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("calling canopy {path}"))?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			return Err(miette::miette!("canopy {path} returned {status}: {body}"));
		}

		response
			.json::<Res>()
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("parsing canopy {path} response"))
	}

	/// Register the backup types this server can run (`POST /backup-capabilities`).
	pub async fn backup_capabilities(&self, types: &[String]) -> Result<()> {
		let (http, url) = self.endpoint_url("/backup-capabilities").await?;
		debug!(%url, ?types, "registering backup capabilities with canopy");
		let response = http
			.post(url)
			.json(&CapabilitiesArgs {
				types: types.to_vec(),
			})
			.send()
			.await
			.into_diagnostic()
			.wrap_err("posting backup capabilities to canopy")?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			return Err(miette::miette!(
				"canopy /backup-capabilities returned {status}: {body}"
			));
		}
		Ok(())
	}

	/// Obtain short-lived S3 credentials for a backup type (`POST /backup-credentials`).
	///
	/// Returns the `credential_process`-shaped creds; the caller translates them
	/// to the container-creds shape for kopia. `412`/`409`/`502` surface as errors.
	pub async fn backup_credentials(
		&self,
		backup_type: &str,
		purpose: BackupPurpose,
	) -> Result<CredentialProcessOutput> {
		let (http, url) = self.endpoint_url("/backup-credentials").await?;
		debug!(%url, backup_type, ?purpose, "requesting backup credentials from canopy");
		let response = http
			.post(url)
			.json(&CredentialsArgs {
				type_: backup_type.to_owned(),
				purpose: Some(purpose),
			})
			.send()
			.await
			.into_diagnostic()
			.wrap_err("posting backup credentials request to canopy")?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			return Err(miette::miette!(
				"canopy /backup-credentials returned {status}: {body}"
			));
		}
		response
			.json::<CredentialProcessOutput>()
			.await
			.into_diagnostic()
			.wrap_err("parsing backup credentials from canopy")
	}

	/// Fetch the S3 repo target (`GET /backup-target`).
	///
	/// `412`/`409` mean the device isn't yet authorised for backups; these map to
	/// [`TargetOutcome::Dormant`] (a benign idle state) rather than an error.
	pub async fn backup_target(&self) -> Result<TargetOutcome> {
		let (http, url) = self.endpoint_url("/backup-target").await?;
		debug!(%url, "fetching backup target from canopy");
		let response = http
			.get(url)
			.send()
			.await
			.into_diagnostic()
			.wrap_err("fetching backup target from canopy")?;

		let status = response.status();
		if status == reqwest::StatusCode::PRECONDITION_FAILED
			|| status == reqwest::StatusCode::CONFLICT
		{
			return Ok(TargetOutcome::Dormant);
		}
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			return Err(miette::miette!(
				"canopy /backup-target returned {status}: {body}"
			));
		}
		let target = response
			.json::<BackupTarget>()
			.await
			.into_diagnostic()
			.wrap_err("parsing backup target from canopy")?;
		Ok(TargetOutcome::Ready(target))
	}

	/// Report a completed backup/restore run (`POST /backup-report`).
	pub async fn backup_report(&self, report: &ReportArgs) -> Result<()> {
		let (http, url) = self.endpoint_url("/backup-report").await?;
		debug!(%url, run_id = %report.run_id, "reporting backup outcome to canopy");
		let response = http
			.post(url)
			.json(report)
			.send()
			.await
			.into_diagnostic()
			.wrap_err("posting backup report to canopy")?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			return Err(miette::miette!(
				"canopy /backup-report returned {status}: {body}"
			));
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
async fn probe_tailscale(
	tailscale_url: &Url,
	make_builder: &ClientBuilderFactory,
) -> Option<reqwest::Client> {
	let host = tailscale_url.host_str()?;

	// The DNS-server and hardcoded-IP discovery below is specific to canopy's
	// own tailnet endpoint; probe any other tailscale URL with plain resolution.
	if host != TAILSCALE_HOST {
		return try_probe(tailscale_url, host, &[], make_builder).await;
	}

	let dns_addrs: Vec<SocketAddr> = tailscale_resolver()
		.lookup_ip("canopy")
		.await
		.ok()
		.map(|addrs| addrs.iter().map(|ip| SocketAddr::new(ip, 443)).collect())
		.unwrap_or_default();
	if !dns_addrs.is_empty()
		&& let Some(client) = try_probe(tailscale_url, host, &dns_addrs, make_builder).await
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
	try_probe(tailscale_url, host, &hardcoded, make_builder).await
}

/// Probe `{tailscale_url}/public/servers`. When `addrs` is non-empty, `host` is
/// resolved to them (the tailnet-discovery override); otherwise plain DNS is used.
async fn try_probe(
	tailscale_url: &Url,
	host: &str,
	addrs: &[SocketAddr],
	make_builder: &ClientBuilderFactory,
) -> Option<reqwest::Client> {
	let mut builder = make_builder()
		.user_agent(user_agent())
		.timeout(TAILSCALE_PROBE_TIMEOUT);
	if !addrs.is_empty() {
		builder = builder.resolve_to_addrs(host, addrs);
	}
	let client = builder.build().ok()?;

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

/// Build a short-lived self-signed client certificate from a P-256 device key
/// PEM and wrap it as a reqwest mTLS [`Identity`].
///
/// Canopy identifies a device by its certificate's public key (SPKI), not by a
/// CA chain, so a fresh self-signed cert from the device key is all that's
/// needed. The same device key drives both the long-running canopy client here
/// and the one-shot `canopy register` enrollment handshake, so they present the
/// same identity to canopy.
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

	fn test_factory() -> ClientBuilderFactory {
		Arc::new(reqwest::Client::builder)
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
	async fn renew_with_mtls_state_swaps_in_fresh_client() {
		// Construct an mTLS-state client directly (no network probe) and renew it.
		let http = build_mtls_http(&test_factory(), TEST_DEVICE_KEY).unwrap();
		let client = CanopyClient {
			base_url: DEFAULT_CANOPY_URL.parse().unwrap(),
			tailscale_url: TAILSCALE_URL.parse().unwrap(),
			device_key: Some(Redacted(TEST_DEVICE_KEY.to_owned())),
			make_builder: test_factory(),
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
			base_url: DEFAULT_CANOPY_URL.parse().unwrap(),
			tailscale_url: TAILSCALE_URL.parse().unwrap(),
			device_key: None,
			make_builder: test_factory(),
			state: RwLock::new(State::Tailscale(http)),
		};
		client.renew().await.expect("renew should be a no-op");
		assert!(client.is_tailscale().await);
	}

	fn mtls_client_against(base: &str) -> CanopyClient {
		let http = build_mtls_http(&test_factory(), TEST_DEVICE_KEY).unwrap();
		CanopyClient {
			base_url: base.parse().unwrap(),
			tailscale_url: TAILSCALE_URL.parse().unwrap(),
			device_key: Some(Redacted(TEST_DEVICE_KEY.to_owned())),
			make_builder: test_factory(),
			state: RwLock::new(State::Mtls(http)),
		}
	}

	struct Captured {
		request_line: String,
		headers: String,
		body: Vec<u8>,
	}

	/// Bind a loopback socket and answer exactly one HTTP request with
	/// `response`, capturing the received request line, headers, and body.
	fn serve_once(response: &'static str) -> (String, std::thread::JoinHandle<Captured>) {
		use std::io::{Read, Write};
		use std::net::TcpListener;

		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		let handle = std::thread::spawn(move || {
			let (mut stream, _) = listener.accept().unwrap();
			let mut buf = Vec::new();
			let mut chunk = [0u8; 1024];
			let header_end = loop {
				if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
					break pos + 4;
				}
				let n = stream.read(&mut chunk).unwrap();
				if n == 0 {
					panic!("connection closed before headers were complete");
				}
				buf.extend_from_slice(&chunk[..n]);
			};

			let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
			let content_length = head
				.lines()
				.find_map(|line| {
					let (name, value) = line.split_once(':')?;
					name.trim()
						.eq_ignore_ascii_case("content-length")
						.then(|| value.trim().parse::<usize>().ok())
						.flatten()
				})
				.unwrap_or(0);

			let mut body = buf[header_end..].to_vec();
			while body.len() < content_length {
				let n = stream.read(&mut chunk).unwrap();
				if n == 0 {
					break;
				}
				body.extend_from_slice(&chunk[..n]);
			}

			stream.write_all(response.as_bytes()).unwrap();
			stream.flush().unwrap();

			let mut lines = head.lines();
			let request_line = lines.next().unwrap_or_default().to_owned();
			let headers = lines.collect::<Vec<_>>().join("\n");
			Captured {
				request_line,
				headers,
				body,
			}
		});
		(base, handle)
	}

	#[derive(Debug, serde::Deserialize, PartialEq)]
	struct Echo {
		ok: bool,
		who: String,
	}

	#[tokio::test]
	async fn request_json_sends_version_and_body_and_parses_response() {
		let (base, handle) = serve_once(
			"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 26\r\n\r\n{\"ok\":true,\"who\":\"device\"}",
		);
		let client = mtls_client_against(&base);

		let payload = serde_json::json!({ "hello": "world" });
		let got: Echo = client
			.request_json(reqwest::Method::POST, "/thing", Some(&payload))
			.await
			.expect("request_json should succeed");

		assert_eq!(
			got,
			Echo {
				ok: true,
				who: "device".into()
			}
		);

		let captured = handle.join().unwrap();
		assert!(
			captured.request_line.starts_with("POST /thing "),
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
		let sent: serde_json::Value = serde_json::from_slice(&captured.body).unwrap();
		assert_eq!(sent, payload);
	}

	#[tokio::test]
	async fn request_json_errors_on_non_success_with_body() {
		let (base, handle) =
			serve_once("HTTP/1.1 418 I'm a teapot\r\nContent-Length: 14\r\n\r\nno coffee here");
		let client = mtls_client_against(&base);

		let err = client
			.request_json::<serde_json::Value>(reqwest::Method::GET, "/brew", None::<&()>)
			.await
			.expect_err("non-2xx should error");
		let msg = err.to_string();
		assert!(msg.contains("/brew"), "expected path in error: {msg}");
		assert!(msg.contains("418"), "expected status in error: {msg}");
		assert!(
			msg.contains("no coffee here"),
			"expected body text in error: {msg}"
		);

		handle.join().unwrap();
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
	fn gzip_bytes_roundtrips() {
		use flate2::read::GzDecoder;
		use std::io::Read;

		let original = br#"{"health":[{"check":"x","result":"passed"}]}"#;
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
}
