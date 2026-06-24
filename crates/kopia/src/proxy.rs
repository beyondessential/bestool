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

use std::{future::Future, net::SocketAddr, pin::Pin, sync::Arc};

use tokio::{net::TcpListener, task::JoinHandle};

mod server;
pub mod sigv4;
pub mod stream;

/// Boxed error used across the proxy's async paths.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

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

	let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
	let addr = listener.local_addr()?;
	let task = tokio::spawn(server::run(listener, config, provider));
	tracing::info!(%addr, "s3 re-signing proxy bound");
	Ok(RunningProxy { addr, task })
}
