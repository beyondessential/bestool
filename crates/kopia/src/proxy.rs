//! Loopback SigV4 re-signing proxy for kopia's S3 backend.
//!
//! kopia talks to this proxy over plain HTTP with meaningless dummy
//! credentials; the proxy discards the dummy signature, re-signs each request
//! with live credentials drawn from a [`CredentialProvider`], and forwards it to
//! real S3 over TLS. A long-running kopia operation (maintenance, a large backup
//! or restore) then outlives any single set of short-lived credentials: each
//! request is signed afresh with whatever the provider currently holds, and a
//! refresh between two requests is invisible to kopia.
//!
//! See the S3P spec for the design.

use std::{
	future::Future,
	net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
	pin::Pin,
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
};

use tokio::{net::TcpListener, task::JoinHandle};

mod server;
pub mod sigv4;
pub mod stream;

/// Boxed error used across the proxy's async paths.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Cumulative byte accounting for a proxy's lifetime, shared with the server
/// task. Since the proxy sees every request, this gives a rough measure of the
/// S3 traffic a run is accountable for.
///
/// `*_raw` counts the full HTTP message — request/status line, headers, and the
/// body as it goes on the wire, including the SigV4 chunk framing kopia adds to
/// streaming uploads. `*_payload` counts only the object data (the decoded
/// body). The difference is protocol overhead.
#[derive(Default)]
pub(crate) struct Traffic {
	sent_raw: AtomicU64,
	sent_payload: AtomicU64,
	received_raw: AtomicU64,
	received_payload: AtomicU64,
}

impl Traffic {
	pub(crate) fn add_sent(&self, raw: u64, payload: u64) {
		self.sent_raw.fetch_add(raw, Ordering::Relaxed);
		self.sent_payload.fetch_add(payload, Ordering::Relaxed);
	}

	pub(crate) fn add_received(&self, raw: u64, payload: u64) {
		self.received_raw.fetch_add(raw, Ordering::Relaxed);
		self.received_payload.fetch_add(payload, Ordering::Relaxed);
	}

	fn snapshot(&self) -> TrafficStats {
		TrafficStats {
			sent_raw: self.sent_raw.load(Ordering::Relaxed),
			sent_payload: self.sent_payload.load(Ordering::Relaxed),
			received_raw: self.received_raw.load(Ordering::Relaxed),
			received_payload: self.received_payload.load(Ordering::Relaxed),
		}
	}
}

/// A point-in-time read of a proxy's [`Traffic`]. All counts are bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrafficStats {
	/// Full request bytes sent upstream (line + headers + framed body).
	pub sent_raw: u64,
	/// Object-data bytes uploaded (decoded, excluding chunk framing).
	pub sent_payload: u64,
	/// Full response bytes received from upstream (line + headers + body).
	pub received_raw: u64,
	/// Object-data bytes downloaded (response bodies).
	pub received_payload: u64,
}

/// A live set of S3 credentials.
#[derive(Clone)]
pub struct Credentials {
	pub access_key: String,
	pub secret_key: String,
	/// `None` for long-lived IAM keys; `Some` for STS / assumed-role credentials.
	pub session_token: Option<String>,
}

/// Source of current credentials, queried by the proxy once per request.
///
/// Implementations cache and refresh ahead of expiry; the call is expected to be
/// cheap when the cached credentials are still valid, and must not block the
/// request path on a network round-trip it can avoid.
pub trait CredentialProvider: Send + Sync + 'static {
	fn credentials(
		&self,
	) -> Pin<Box<dyn Future<Output = Result<Credentials, BoxError>> + Send + '_>>;
}

/// A fixed set of credentials — for short operations and tests.
pub struct StaticCredentialProvider(pub Credentials);

impl CredentialProvider for StaticCredentialProvider {
	fn credentials(
		&self,
	) -> Pin<Box<dyn Future<Output = Result<Credentials, BoxError>> + Send + '_>> {
		let creds = self.0.clone();
		Box::pin(async move { Ok(creds) })
	}
}

/// The upstream target the proxy re-signs requests for.
#[derive(Clone)]
pub struct S3ProxyConfig {
	/// Upstream base URL including scheme, e.g.
	/// `https://s3.ap-southeast-2.amazonaws.com`.
	pub upstream: String,
	/// Host (and port, if any) used in the `Host` header and the signature,
	/// e.g. `s3.ap-southeast-2.amazonaws.com`.
	pub upstream_host: String,
	/// AWS region for the credential scope.
	pub region: String,
}

/// A running proxy. Dropping it (or calling [`shutdown`](Self::shutdown)) stops
/// the listener.
pub struct RunningProxy {
	addr: SocketAddr,
	task: JoinHandle<()>,
	traffic: Arc<Traffic>,
}

impl RunningProxy {
	/// The loopback address kopia should be pointed at.
	pub fn addr(&self) -> SocketAddr {
		self.addr
	}

	/// `host:port` form for kopia's `--endpoint` (TLS is disabled on this leg).
	pub fn endpoint(&self) -> String {
		self.addr.to_string()
	}

	/// Bytes sent to and received from upstream S3 over this proxy's lifetime.
	pub fn traffic(&self) -> TrafficStats {
		self.traffic.snapshot()
	}

	/// Stop serving.
	pub async fn shutdown(self) {
		self.task.abort();
	}
}

impl Drop for RunningProxy {
	fn drop(&mut self) {
		self.task.abort();
	}
}

/// Bind an ephemeral loopback port and start serving. Cheap — spawn one per
/// operation, each with its own provider and upstream target.
pub async fn spawn(
	config: S3ProxyConfig,
	provider: Arc<dyn CredentialProvider>,
) -> std::io::Result<RunningProxy> {
	// The TLS upstream leg needs a rustls crypto provider; install aws-lc-rs's
	// (the workspace standard) if no process default is set yet (idempotent,
	// ignores "already installed").
	let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

	let listener = bind_loopback().await?;
	let addr = listener.local_addr()?;
	let traffic = Arc::new(Traffic::default());
	let task = tokio::spawn(server::run(listener, config, provider, traffic.clone()));
	tracing::info!(%addr, "s3 re-signing proxy bound");
	Ok(RunningProxy {
		addr,
		task,
		traffic,
	})
}

/// Bind an ephemeral loopback port, preferring IPv6 but falling back to IPv4.
///
/// IPv6-only hosts (such as some Kubernetes clusters) can't bind `127.0.0.1`,
/// and IPv4-only hosts can't bind `::1`, so try each in turn.
async fn bind_loopback() -> std::io::Result<TcpListener> {
	let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0);
	match TcpListener::bind(v6).await {
		Ok(listener) => Ok(listener),
		Err(v6_err) => {
			let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
			TcpListener::bind(v4).await.map_err(|v4_err| {
				std::io::Error::other(format!(
					"could not bind loopback: IPv6 ({v6_err}), IPv4 ({v4_err})"
				))
			})
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn bind_loopback_yields_loopback_endpoint() {
		let listener = bind_loopback().await.expect("bind loopback");
		let addr = listener.local_addr().expect("local addr");
		assert!(addr.ip().is_loopback(), "expected loopback, got {addr}");
		assert_ne!(addr.port(), 0, "expected an ephemeral port");
	}
}
