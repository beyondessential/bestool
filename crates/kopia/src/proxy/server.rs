//! Accept loop and per-request re-signing handler.

use std::{
	convert::Infallible,
	pin::Pin,
	sync::Arc,
	task::{Context, Poll},
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{
	Request, Response, StatusCode,
	body::{Body, Frame, Incoming, SizeHint},
	header::{HeaderMap, HeaderName, HeaderValue},
};
use hyper_util::{
	client::legacy::{Client, connect::HttpConnector},
	rt::{TokioExecutor, TokioIo},
};
use tokio::net::TcpListener;

use super::{BoxError, CredentialProvider, S3ProxyConfig, Traffic, sigv4, stream::ChunkResigner};

type ReqBody = BoxBody<Bytes, BoxError>;
type ResBody = BoxBody<Bytes, BoxError>;
type HttpsClient = Client<hyper_rustls::HttpsConnector<HttpConnector>, ReqBody>;

/// Request headers we never forward upstream: `host`/`authorization` are
/// regenerated, `content-length` is re-derived from the (length-preserving)
/// body, and the rest are hop-by-hop.
const DROP_REQUEST_HEADERS: &[&str] = &[
	"host",
	"authorization",
	"content-length",
	"connection",
	"accept-encoding",
	"transfer-encoding",
	"te",
];

/// Response headers we don't pass back: framing is re-derived from the body.
const DROP_RESPONSE_HEADERS: &[&str] = &["connection", "transfer-encoding", "content-length"];

struct State {
	config: S3ProxyConfig,
	provider: Arc<dyn CredentialProvider>,
	client: HttpsClient,
	traffic: Arc<Traffic>,
}

pub(super) async fn run(
	listener: TcpListener,
	config: S3ProxyConfig,
	provider: Arc<dyn CredentialProvider>,
	traffic: Arc<Traffic>,
) {
	let https = hyper_rustls::HttpsConnectorBuilder::new()
		.with_webpki_roots()
		.https_or_http()
		.enable_http1()
		.build();
	let client: HttpsClient = Client::builder(TokioExecutor::new()).build(https);
	let state = Arc::new(State {
		config,
		provider,
		client,
		traffic,
	});

	loop {
		let (stream, _) = match listener.accept().await {
			Ok(pair) => pair,
			Err(e) => {
				tracing::warn!(error = %e, "proxy accept failed");
				continue;
			}
		};
		let state = state.clone();
		tokio::spawn(async move {
			let service = hyper::service::service_fn(move |req| {
				let state = state.clone();
				async move { Ok::<_, Infallible>(handle(state, req).await) }
			});
			if let Err(e) = hyper::server::conn::http1::Builder::new()
				.serve_connection(TokioIo::new(stream), service)
				.await
			{
				tracing::debug!(error = %e, "proxy connection ended");
			}
		});
	}
}

async fn handle(state: Arc<State>, req: Request<Incoming>) -> Response<ResBody> {
	match proxy(&state, req).await {
		Ok(resp) => resp,
		Err(e) => {
			tracing::warn!(error = %e, "proxy request failed");
			text(StatusCode::BAD_GATEWAY, format!("proxy error: {e}"))
		}
	}
}

async fn proxy(state: &State, req: Request<Incoming>) -> Result<Response<ResBody>, BoxError> {
	let creds = state
		.provider
		.credentials()
		.await
		.map_err(|e| -> BoxError { format!("credential refresh failed: {e}").into() })?;

	let (parts, body) = req.into_parts();
	let method = parts.method;
	let path = parts.uri.path().to_string();
	let query = parts.uri.query().unwrap_or("").to_string();
	let headers = parts.headers;

	// Reuse kopia's x-amz-date so the credential scope and date stay consistent.
	let amz_date = headers
		.get("x-amz-date")
		.and_then(|v| v.to_str().ok())
		.ok_or_else(|| -> BoxError { "request missing x-amz-date".into() })?
		.to_string();
	let date_stamp = amz_date
		.get(..8)
		.ok_or_else(|| -> BoxError { "malformed x-amz-date".into() })?
		.to_string();
	let scope = sigv4::scope(&date_stamp, &state.config.region, "s3");
	let signing_key =
		sigv4::signing_key(&creds.secret_key, &date_stamp, &state.config.region, "s3");

	let incoming_sha = headers
		.get("x-amz-content-sha256")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.to_string();
	let streaming = incoming_sha == sigv4::STREAMING_PAYLOAD;

	// Headers forwarded upstream: everything kopia sent bar the dropped set,
	// with the upstream host and (for STS) the session token.
	let mut fwd: Vec<(String, String)> = Vec::new();
	for (name, value) in headers.iter() {
		let n = name.as_str().to_ascii_lowercase();
		if DROP_REQUEST_HEADERS.contains(&n.as_str()) {
			continue;
		}
		if let Ok(v) = value.to_str() {
			fwd.push((n, v.to_string()));
		}
	}
	fwd.push(("host".into(), state.config.upstream_host.clone()));
	fwd.retain(|(n, _)| n != "x-amz-security-token");
	if let Some(token) = &creds.session_token {
		fwd.push(("x-amz-security-token".into(), token.clone()));
	}

	// The subset that goes into the signature.
	let signed: Vec<(String, String)> = fwd
		.iter()
		.filter(|(n, _)| is_signed_header(n))
		.cloned()
		.collect();
	let (canonical_headers, signed_headers) = sigv4::canonical_headers(&signed);

	let hashed_payload = if streaming {
		sigv4::STREAMING_PAYLOAD.to_string()
	} else if incoming_sha.is_empty() {
		sigv4::EMPTY_SHA256.to_string()
	} else {
		incoming_sha.clone()
	};

	let canonical_request = sigv4::canonical_request(
		method.as_str(),
		&sigv4::canonical_uri(&path),
		&sigv4::canonical_query(&query),
		&canonical_headers,
		&signed_headers,
		&hashed_payload,
	);
	let string_to_sign = sigv4::string_to_sign(&amz_date, &scope, &canonical_request);
	let seed = sigv4::signature(&signing_key, &string_to_sign);
	let authorization = format!(
		"AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={seed}",
		creds.access_key
	);

	// Account the request: `content-length` is the body as it goes on the wire
	// (the chunk-framed length for a streaming upload); the payload is the decoded
	// object data — `x-amz-decoded-content-length` when streaming, else the body.
	let body_len = header_u64(&headers, "content-length");
	let payload_len = if streaming {
		header_u64(&headers, "x-amz-decoded-content-length")
	} else {
		body_len
	};
	let request_overhead = request_overhead(method.as_str(), &path, &query, &fwd, &authorization);
	state
		.traffic
		.add_sent(request_overhead + body_len, payload_len);

	let upstream_body: ReqBody = if streaming {
		let exact_len: u64 = headers
			.get("content-length")
			.and_then(|v| v.to_str().ok())
			.and_then(|s| s.parse().ok())
			.ok_or_else(|| -> BoxError { "streaming PUT without content-length".into() })?;
		let resigner =
			ChunkResigner::new(signing_key, amz_date.clone(), scope.clone(), seed.clone());
		ResignBody {
			inner: body,
			resigner,
			exact_len,
		}
		.boxed()
	} else {
		body.map_err(|e| -> BoxError { Box::new(e) }).boxed()
	};

	let url = if query.is_empty() {
		format!("{}{}", state.config.upstream, path)
	} else {
		format!("{}{}?{}", state.config.upstream, path, query)
	};

	let mut builder = Request::builder().method(method).uri(url);
	for (n, v) in &fwd {
		if n == "host" {
			continue; // hyper sets Host from the URL authority
		}
		if let (Ok(name), Ok(value)) = (
			HeaderName::try_from(n.as_str()),
			HeaderValue::try_from(v.as_str()),
		) {
			builder = builder.header(name, value);
		}
	}
	builder = builder.header("authorization", authorization);
	let upstream_req = builder
		.body(upstream_body)
		.map_err(|e| -> BoxError { Box::new(e) })?;

	let resp = state
		.client
		.request(upstream_req)
		.await
		.map_err(|e| -> BoxError { Box::new(e) })?;

	let status = resp.status();
	if !status.is_success() {
		tracing::debug!(%status, %path, "upstream returned non-2xx");
	}
	let (parts, body) = resp.into_parts();
	// Account the response line + headers now; the body is counted as it streams.
	state
		.traffic
		.add_received(response_overhead(status, &parts.headers), 0);
	let mut out = Response::builder().status(status);
	for (name, value) in parts.headers.iter() {
		if DROP_RESPONSE_HEADERS.contains(&name.as_str()) {
			continue;
		}
		out = out.header(name, value);
	}
	let body: ResBody = CountingBody {
		inner: body.map_err(|e| -> BoxError { Box::new(e) }).boxed(),
		traffic: state.traffic.clone(),
	}
	.boxed();
	out.body(body).map_err(|e| -> BoxError { Box::new(e) })
}

fn is_signed_header(name: &str) -> bool {
	name == "host"
		|| name.starts_with("x-amz-")
		|| name == "content-type"
		|| name == "content-md5"
		|| name == "content-encoding"
}

fn text(status: StatusCode, msg: String) -> Response<ResBody> {
	let body = Full::new(Bytes::from(msg))
		.map_err(|e: Infallible| match e {})
		.boxed();
	Response::builder()
		.status(status)
		.body(body)
		.expect("static response builds")
}

/// Wraps the incoming chunked PUT body, re-signing each chunk on the fly. The
/// re-encoded body is the same byte length, so [`SizeHint::with_exact`] lets
/// hyper send a real `Content-Length` rather than chunked transfer-encoding,
/// which streaming SigV4 requires.
struct ResignBody {
	inner: Incoming,
	resigner: ChunkResigner,
	exact_len: u64,
}

impl Body for ResignBody {
	type Data = Bytes;
	type Error = BoxError;

	fn poll_frame(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		let this = self.get_mut();
		loop {
			match Pin::new(&mut this.inner).poll_frame(cx) {
				Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
					Ok(data) => match this.resigner.push(&data) {
						Ok(out) if out.is_empty() => continue,
						Ok(out) => return Poll::Ready(Some(Ok(Frame::data(Bytes::from(out))))),
						Err(e) => return Poll::Ready(Some(Err(Box::new(e)))),
					},
					// Trailers — not expected on a chunked PUT; ignore.
					Err(_) => continue,
				},
				Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(Box::new(e)))),
				Poll::Ready(None) => return Poll::Ready(None),
				Poll::Pending => return Poll::Pending,
			}
		}
	}

	fn size_hint(&self) -> SizeHint {
		SizeHint::with_exact(self.exact_len)
	}
}

/// Wraps the upstream response body, tallying every byte received into the
/// proxy's [`Traffic`]. A response body is object data with no extra framing, so
/// it counts as both raw and payload.
struct CountingBody {
	inner: ResBody,
	traffic: Arc<Traffic>,
}

impl Body for CountingBody {
	type Data = Bytes;
	type Error = BoxError;

	fn poll_frame(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
		let this = self.get_mut();
		match Pin::new(&mut this.inner).poll_frame(cx) {
			Poll::Ready(Some(Ok(frame))) => {
				if let Some(data) = frame.data_ref() {
					let n = data.len() as u64;
					this.traffic.add_received(n, n);
				}
				Poll::Ready(Some(Ok(frame)))
			}
			other => other,
		}
	}

	fn size_hint(&self) -> SizeHint {
		self.inner.size_hint()
	}
}

/// A request/response header's value parsed as a `u64`, or 0 if absent/unparsable.
fn header_u64(headers: &HeaderMap, name: &str) -> u64 {
	headers
		.get(name)
		.and_then(|v| v.to_str().ok())
		.and_then(|s| s.parse().ok())
		.unwrap_or(0)
}

/// Rough wire size of the request line + forwarded headers + authorization
/// (`name: value\r\n` per header, trailing blank line). An estimate: it doesn't
/// model HTTP/2 or TLS, which is fine for the per-deployment traffic accounting
/// this feeds.
fn request_overhead(
	method: &str,
	path: &str,
	query: &str,
	fwd: &[(String, String)],
	authorization: &str,
) -> u64 {
	let query_len = if query.is_empty() { 0 } else { 1 + query.len() };
	let line = method.len() + 1 + path.len() + query_len + " HTTP/1.1\r\n".len();
	let headers: usize = fwd.iter().map(|(n, v)| n.len() + 2 + v.len() + 2).sum();
	let auth = "authorization".len() + 2 + authorization.len() + 2;
	(line + headers + auth + 2) as u64
}

/// Rough wire size of the response status line + headers.
fn response_overhead(status: StatusCode, headers: &HeaderMap) -> u64 {
	let reason = status.canonical_reason().map_or(0, str::len);
	let line = "HTTP/1.1 ".len() + 3 + 1 + reason + 2;
	let headers: usize = headers
		.iter()
		.map(|(n, v)| n.as_str().len() + 2 + v.as_bytes().len() + 2)
		.sum();
	(line + headers + 2) as u64
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn header_u64_parses_or_zeroes() {
		let mut headers = HeaderMap::new();
		headers.insert("content-length", HeaderValue::from_static("1234"));
		assert_eq!(header_u64(&headers, "content-length"), 1234);
		assert_eq!(header_u64(&headers, "x-amz-decoded-content-length"), 0);
	}

	#[test]
	fn request_overhead_sums_line_headers_and_auth() {
		// "GET /p HTTP/1.1\r\n" (17) + "host: h\r\n" (9) + "authorization: a\r\n"
		// (18) + trailing "\r\n" (2) = 46.
		let fwd = vec![("host".to_string(), "h".to_string())];
		assert_eq!(request_overhead("GET", "/p", "", &fwd, "a"), 46);
	}

	#[test]
	fn request_overhead_includes_query() {
		let fwd = vec![];
		// "GET /p?x=1 HTTP/1.1\r\n" (21) + "authorization: a\r\n" (18) + "\r\n" (2).
		assert_eq!(request_overhead("GET", "/p", "x=1", &fwd, "a"), 41);
	}

	#[test]
	fn response_overhead_sums_status_line_and_headers() {
		let mut headers = HeaderMap::new();
		headers.insert("content-type", HeaderValue::from_static("text/plain"));
		// "HTTP/1.1 200 OK\r\n" (17) + "content-type: text/plain\r\n" (26) + "\r\n" (2).
		assert_eq!(response_overhead(StatusCode::OK, &headers), 45);
	}
}
