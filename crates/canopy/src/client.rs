use std::{fmt, io::Write};

use bytes::Bytes;
use flate2::{Compression, write::GzEncoder};
use miette::{IntoDiagnostic, Result, WrapErr};
use reqwest::Url;

use crate::{
	reqwest_transport::ReqwestTransport,
	transport::{CanopyResponse, CanopyTransport},
};

/// A non-2xx response from a canopy endpoint.
///
/// The generated endpoint methods return this (wrapped in a [`miette::Report`])
/// on any non-success status; downcast the report to it to branch on the code,
/// e.g. [`TargetOutcome::from_result`](crate::TargetOutcome::from_result) maps a
/// backup-target `412`/`409` to a dormant device.
#[derive(Debug, Clone)]
pub struct CanopyHttpError {
	/// HTTP status returned by canopy.
	pub status: reqwest::StatusCode,
	/// The endpoint path that was called (mTLS-mode form, e.g. `/backup-target`).
	pub path: String,
	/// Response body, best-effort (empty if it couldn't be read).
	pub body: String,
}

impl fmt::Display for CanopyHttpError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"canopy {} returned {}: {}",
			self.path, self.status, self.body
		)
	}
}

impl std::error::Error for CanopyHttpError {}
impl miette::Diagnostic for CanopyHttpError {}

/// Typed client for canopy's API.
///
/// Carries one generated method per endpoint (see [`schema`](crate::schema)),
/// taking and returning the wire types from canopy's OpenAPI document. Those
/// methods handle the parts that don't vary by endpoint — gzipping the request
/// body, mapping a non-2xx to [`CanopyHttpError`], parsing the response — and
/// hand the actual HTTP over to a [`CanopyTransport`].
///
/// The transport defaults to [`ReqwestTransport`], which picks between canopy's
/// tailscale and mTLS auth paths; [`Self::new`] and [`Self::with_urls`] build
/// one, so the common case never names it. A caller that reaches canopy some
/// other way — through a proxy that isn't a plain HTTP proxy, say — implements
/// [`CanopyTransport`] itself and constructs the client with
/// [`Self::with_transport`], keeping every generated method and wire type.
pub struct CanopyClient<T = ReqwestTransport> {
	transport: T,
}

impl<T> fmt::Debug for CanopyClient<T> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CanopyClient").finish_non_exhaustive()
	}
}

impl CanopyClient<ReqwestTransport> {
	/// Build a canopy client against the default public
	/// ([`DEFAULT_CANOPY_URL`](crate::DEFAULT_CANOPY_URL)) and tailscale
	/// ([`TAILSCALE_URL`](crate::TAILSCALE_URL)) endpoints. Use [`Self::with_urls`]
	/// to override them.
	///
	/// Probes the tailscale endpoint first; if reachable, uses it. Otherwise, if
	/// a device key PEM is provided, builds an mTLS client. Returns `Ok(None)` if
	/// neither path is available.
	///
	/// `make_builder` supplies the base [`reqwest::ClientBuilder`] — see
	/// [`ClientBuilderFactory`](crate::ClientBuilderFactory).
	pub async fn new(
		device_key_pem: Option<&str>,
		make_builder: impl Fn() -> reqwest::ClientBuilder + Send + Sync + 'static,
	) -> Result<Option<Self>> {
		Self::with_urls(
			crate::DEFAULT_CANOPY_URL
				.parse()
				.expect("default canopy URL is valid"),
			crate::TAILSCALE_URL
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
		Ok(
			ReqwestTransport::new(base_url, tailscale_url, device_key_pem, make_builder)
				.await?
				.map(Self::with_transport),
		)
	}

	/// Returns true if the client is currently using the tailscale path.
	pub async fn is_tailscale(&self) -> bool {
		self.transport.is_tailscale().await
	}

	/// Re-probe tailscale and swap modes if the picture has changed.
	///
	/// Intended to be called when the daemon receives a reload signal.
	pub async fn refresh(&self) -> Result<()> {
		self.transport.refresh().await
	}

	/// Rebuild the underlying HTTP client with a fresh certificate.
	///
	/// No-op in tailscale mode (no cert to rotate). In mTLS mode, atomically
	/// replaces the live client; in-flight requests continue with the old
	/// client until they complete.
	pub async fn renew(&self) -> Result<()> {
		self.transport.renew().await
	}

	/// GET a path, routed via tailscale when available, returning the raw response.
	///
	/// Escape hatch behind the generated endpoint methods; needs the `raw-requests`
	/// feature. In tailscale mode the request goes to `{tailscale_url}{tailscale_path}`
	/// (typically `/public/...`); in mTLS mode to `{base_url}{mtls_path}`.
	#[cfg(feature = "raw-requests")]
	pub async fn get(&self, tailscale_path: &str, mtls_path: &str) -> Result<reqwest::Response> {
		self.transport.get(tailscale_path, mtls_path).await
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
		self.transport.request(method, path).await
	}
}

impl<T: CanopyTransport> CanopyClient<T> {
	/// Build a canopy client over a caller-supplied transport.
	///
	/// Everything the client does above the HTTP layer — the generated endpoint
	/// methods, the wire types, gzipping, error mapping — works the same on any
	/// [`CanopyTransport`]. Use this to route canopy calls through something
	/// other than the default [`ReqwestTransport`], e.g. a proxy that speaks a
	/// dialect of its own, or a stub in tests.
	pub fn with_transport(transport: T) -> Self {
		Self { transport }
	}

	/// The transport this client sends through.
	pub fn transport(&self) -> &T {
		&self.transport
	}

	/// Send a request to `path` through the transport, gzipping the JSON body
	/// when there is one.
	///
	/// A non-success status becomes a [`CanopyHttpError`] (downcast the returned
	/// report to inspect the status — e.g. [`TargetOutcome::from_result`]). This
	/// is the shared core behind the generated endpoint methods.
	///
	/// [`TargetOutcome::from_result`]: crate::TargetOutcome::from_result
	async fn send_call<B: serde::Serialize + ?Sized>(
		&self,
		method: reqwest::Method,
		path: &str,
		body: Option<&B>,
	) -> Result<CanopyResponse> {
		let mut request = http::Request::builder().method(method).uri(path);
		let body = match body {
			Some(body) => {
				let raw = serde_json::to_vec(body)
					.into_diagnostic()
					.wrap_err_with(|| format!("serialising canopy {path} body"))?;
				let compressed = gzip_bytes(&raw)
					.into_diagnostic()
					.wrap_err_with(|| format!("gzipping canopy {path} body"))?;
				request = request
					.header(reqwest::header::CONTENT_TYPE, "application/json")
					.header(reqwest::header::CONTENT_ENCODING, "gzip");
				Bytes::from(compressed)
			}
			None => Bytes::new(),
		};

		let request = request
			.body(body)
			.into_diagnostic()
			.wrap_err_with(|| format!("building canopy {path} request"))?;

		let response = self
			.transport
			.call(request)
			.await
			.wrap_err_with(|| format!("calling canopy {path}"))?;

		let status = response.status();
		if !status.is_success() {
			return Err(miette::Report::new(CanopyHttpError {
				status,
				path: path.to_owned(),
				body: String::from_utf8_lossy(response.body()).into_owned(),
			}));
		}
		Ok(response)
	}

	/// Call an endpoint and parse its JSON response. Backs the generated methods.
	pub(crate) async fn call_json<B, R>(
		&self,
		method: reqwest::Method,
		path: &str,
		body: Option<&B>,
	) -> Result<R>
	where
		B: serde::Serialize + ?Sized,
		R: serde::de::DeserializeOwned,
	{
		let response = self.send_call(method, path, body).await?;
		serde_json::from_slice(response.body())
			.into_diagnostic()
			.wrap_err_with(|| format!("parsing canopy {path} response"))
	}

	/// Call an endpoint that returns no body. Backs the generated methods.
	pub(crate) async fn call_empty<B: serde::Serialize + ?Sized>(
		&self,
		method: reqwest::Method,
		path: &str,
		body: Option<&B>,
	) -> Result<()> {
		self.send_call(method, path, body).await.map(drop)
	}

	/// Call an arbitrary canopy endpoint and parse its JSON response.
	///
	/// Escape hatch behind the generated endpoint methods; needs the `raw-requests`
	/// feature. Prefer a generated method where one exists. When passing no body,
	/// pin the inference with a turbofish, e.g. `None::<&()>`. The body is gzipped,
	/// like every canopy request.
	#[cfg(feature = "raw-requests")]
	pub async fn request_json<Res: serde::de::DeserializeOwned>(
		&self,
		method: reqwest::Method,
		path: &str,
		body: Option<&(impl serde::Serialize + ?Sized)>,
	) -> Result<Res> {
		self.call_json(method, path, body).await
	}
}

fn gzip_bytes(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
	let mut encoder = GzEncoder::new(Vec::with_capacity(bytes.len() / 2), Compression::default());
	encoder.write_all(bytes)?;
	encoder.finish()
}

#[cfg(test)]
mod tests {
	use std::sync::Mutex;

	use crate::{
		DEFAULT_CANOPY_URL,
		test_support::{closed_url, serve_once},
		transport::CanopyRequest,
	};

	use super::*;

	fn mtls_client_against(base: &str) -> CanopyClient {
		CanopyClient::with_transport(ReqwestTransport::mtls_for_tests(base))
	}

	#[derive(Debug, serde::Deserialize, PartialEq)]
	struct Echo {
		ok: bool,
		who: String,
	}

	/// The default-transport constructor and the auth-path methods that delegate
	/// to it; what the transport itself decides is covered in `reqwest_transport`.
	#[tokio::test]
	async fn with_urls_builds_on_the_default_transport() {
		let (tailnet, _server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]");
		let client = CanopyClient::with_urls(
			DEFAULT_CANOPY_URL.parse().unwrap(),
			tailnet.parse().unwrap(),
			None,
			reqwest::Client::builder,
		)
		.await
		.expect("keyless build should not error")
		.expect("a reachable tailnet is an auth path in its own right");
		assert!(client.is_tailscale().await);
		client.renew().await.expect("renew should be a no-op");
	}

	#[tokio::test]
	async fn with_urls_yields_no_client_without_an_auth_path() {
		let client = CanopyClient::with_urls(
			DEFAULT_CANOPY_URL.parse().unwrap(),
			closed_url().parse().unwrap(),
			None,
			reqwest::Client::builder,
		)
		.await
		.expect("keyless build should not error");
		assert!(client.is_none());
	}

	#[tokio::test]
	async fn call_json_gzips_body_sets_user_agent_and_parses_response() {
		let (base, handle) = serve_once(
			"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 26\r\n\r\n{\"ok\":true,\"who\":\"device\"}",
		);
		let client = mtls_client_against(&base);

		let payload = serde_json::json!({ "hello": "world" });
		let got: Echo = client
			.call_json(reqwest::Method::POST, "/thing", Some(&payload))
			.await
			.expect("call_json should succeed");

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
		let headers = captured.headers.to_ascii_lowercase();
		assert!(
			headers.contains("user-agent: bestool-canopy/"),
			"missing canopy user-agent in:\n{}",
			captured.headers
		);
		assert!(
			headers.contains("content-encoding: gzip"),
			"body should be gzipped:\n{}",
			captured.headers
		);
		// The body is gzipped on the wire; decompress before comparing.
		let sent: serde_json::Value = serde_json::from_slice(&gunzip(&captured.body)).unwrap();
		assert_eq!(sent, payload);
	}

	#[tokio::test]
	async fn call_json_errors_on_non_success_with_body() {
		let (base, handle) =
			serve_once("HTTP/1.1 418 I'm a teapot\r\nContent-Length: 14\r\n\r\nno coffee here");
		let client = mtls_client_against(&base);

		let err = client
			.call_json::<(), serde_json::Value>(reqwest::Method::GET, "/brew", None::<&()>)
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

	/// A transport that records what it was handed and replays a canned response,
	/// standing in for a caller's own (proxying, in-process, …) implementation.
	#[derive(Default)]
	struct StubTransport {
		seen: Mutex<Vec<CanopyRequest>>,
		response: Option<CanopyResponse>,
	}

	impl StubTransport {
		fn responding(status: u16, body: &str) -> Self {
			Self {
				seen: Mutex::default(),
				response: Some(
					http::Response::builder()
						.status(status)
						.body(Bytes::copy_from_slice(body.as_bytes()))
						.unwrap(),
				),
			}
		}

		fn took(&self) -> Vec<CanopyRequest> {
			std::mem::take(&mut *self.seen.lock().unwrap())
		}
	}

	#[async_trait::async_trait]
	impl CanopyTransport for StubTransport {
		async fn call(&self, request: CanopyRequest) -> Result<CanopyResponse> {
			self.seen.lock().unwrap().push(request);
			match &self.response {
				Some(response) => {
					let mut clone = http::Response::new(response.body().clone());
					*clone.status_mut() = response.status();
					*clone.headers_mut() = response.headers().clone();
					Ok(clone)
				}
				None => Err(miette::miette!("this transport cannot reach canopy")),
			}
		}
	}

	#[tokio::test]
	async fn a_custom_transport_carries_the_typed_calls() {
		let client = CanopyClient::with_transport(StubTransport::responding(
			200,
			r#"{"ok":true,"who":"stub"}"#,
		));

		let payload = serde_json::json!({ "hello": "world" });
		let got: Echo = client
			.call_json(reqwest::Method::POST, "/thing", Some(&payload))
			.await
			.expect("the typed machinery should run on any transport");
		assert_eq!(
			got,
			Echo {
				ok: true,
				who: "stub".into()
			}
		);

		let seen = client.transport().took();
		let [request] = &seen[..] else {
			panic!("expected exactly one request, got {}", seen.len());
		};
		assert_eq!(request.method(), reqwest::Method::POST);
		// The transport resolves the path against whatever base it uses, so it's
		// handed the endpoint path on its own.
		assert_eq!(request.uri(), "/thing");
		assert_eq!(
			request.headers().get(reqwest::header::CONTENT_ENCODING),
			Some(&reqwest::header::HeaderValue::from_static("gzip"))
		);
		assert_eq!(
			serde_json::from_slice::<serde_json::Value>(&gunzip(request.body())).unwrap(),
			payload
		);
	}

	#[tokio::test]
	async fn a_custom_transport_sees_generated_endpoint_methods() {
		// A generated method (`GET /servers`) over a caller-supplied transport:
		// path, verb, and the empty body all come from the OpenAPI document.
		let client = CanopyClient::with_transport(StubTransport::responding(200, "[]"));
		let servers = client
			.servers()
			.await
			.expect("generated methods work on any transport");
		assert!(servers.is_empty());

		let seen = client.transport().took();
		let [request] = &seen[..] else {
			panic!("expected exactly one request, got {}", seen.len());
		};
		assert_eq!(request.method(), reqwest::Method::GET);
		assert_eq!(request.uri(), "/servers");
		assert!(request.body().is_empty(), "a GET should carry no body");
		assert!(
			request
				.headers()
				.get(reqwest::header::CONTENT_TYPE)
				.is_none(),
			"a bodyless request shouldn't claim a content type"
		);
	}

	#[tokio::test]
	async fn a_custom_transport_maps_non_success_to_canopy_http_error() {
		let client =
			CanopyClient::with_transport(StubTransport::responding(412, "device is dormant"));
		let err = client
			.backup_target()
			.await
			.expect_err("412 is not a success");
		let http_err = err
			.downcast_ref::<CanopyHttpError>()
			.expect("non-2xx from any transport surfaces as CanopyHttpError");
		assert_eq!(http_err.status, reqwest::StatusCode::PRECONDITION_FAILED);
		assert_eq!(http_err.path, "/backup-target");
		assert_eq!(http_err.body, "device is dormant");
	}

	#[tokio::test]
	async fn a_custom_transport_error_is_reported_with_the_path() {
		let client = CanopyClient::with_transport(StubTransport::default());
		let err = client.tags().await.expect_err("the stub always fails");
		let chain = format!("{err:?}");
		assert!(chain.contains("/tags"), "expected path in report: {chain}");
		assert!(
			chain.contains("cannot reach canopy"),
			"expected the transport's own error in report: {chain}"
		);
	}

	#[tokio::test]
	async fn a_boxed_transport_is_a_transport() {
		// So a caller can pick a transport at runtime without naming its type.
		let client: CanopyClient<Box<dyn CanopyTransport>> =
			CanopyClient::with_transport(Box::new(StubTransport::responding(200, "[]")));
		assert!(client.servers().await.unwrap().is_empty());
	}

	fn gunzip(bytes: &[u8]) -> Vec<u8> {
		use flate2::read::GzDecoder;
		use std::io::Read as _;

		let mut out = Vec::new();
		GzDecoder::new(bytes)
			.read_to_end(&mut out)
			.expect("body should be valid gzip");
		out
	}

	#[test]
	fn gzip_bytes_roundtrips() {
		let original = br#"{"health":[{"check":"x","result":"passed"}]}"#;
		let compressed = gzip_bytes(original).expect("gzip should succeed");
		assert!(
			compressed.starts_with(&[0x1f, 0x8b]),
			"expected gzip magic bytes"
		);
		assert_eq!(gunzip(&compressed), original);
	}
}
