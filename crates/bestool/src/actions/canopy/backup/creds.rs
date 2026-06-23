//! Loopback container-credentials endpoint for the kopia subprocess.
//!
//! kopia's minio-go S3 backend can't use `credential_process`; it polls an
//! ECS-style container-credentials endpoint and self-refreshes. So the driver
//! runs a tiny loopback HTTP server: each run [`CredsServer::lease`]s a bearer
//! token, points kopia at the endpoint via the `AWS_CONTAINER_CREDENTIALS_FULL_URI`
//! and `AWS_CONTAINER_AUTHORIZATION_TOKEN` env vars, and the handler serves creds
//! (fetched from Canopy's `/backup-credentials`, translated to the container-creds
//! shape) refreshing them as they near expiry. The token is deregistered when the
//! lease drops, so a leaked token stops working once the run ends.
//!
//! Verified against kopia 0.23.1 + minio-go 7.2.0: `GET` with a raw
//! `Authorization: <token>` header; reply `200` + JSON
//! `{AccessKeyId, SecretAccessKey, Token, Expiration}` (`Token`, not
//! `SessionToken`); the host must be loopback (we bind `127.0.0.1`).

use std::{
	collections::HashMap,
	net::{Ipv4Addr, SocketAddr},
	sync::{Arc, Mutex},
};

use axum::{
	Json, Router,
	extract::State,
	http::{HeaderMap, StatusCode, header::AUTHORIZATION},
	routing::get,
};
use bestool_canopy::{BackupCredentials, ContainerCreds};
use futures::future::BoxFuture;
use jiff::{Timestamp, ToSpan};
use miette::{Context as _, IntoDiagnostic as _, Result};
use tokio::{net::TcpListener, sync::Mutex as AsyncMutex};
use tracing::error;
use uuid::Uuid;

/// Refresh creds as they near expiry, this far ahead of `Expiration`.
const REFRESH_MARGIN_MINUTES: i64 = 2;

/// Fetches a fresh set of creds from Canopy. Boxed so the handler is testable
/// without a live `CanopyClient`.
pub type Refresher =
	Arc<dyn Fn() -> BoxFuture<'static, std::result::Result<BackupCredentials, String>> + Send + Sync>;

struct Lease {
	refresh: Refresher,
	cached: AsyncMutex<Option<BackupCredentials>>,
}

type Registry = Arc<Mutex<HashMap<String, Arc<Lease>>>>;

/// A running loopback creds endpoint. Cheaply cloneable.
#[derive(Clone)]
pub struct CredsServer {
	base_uri: String,
	registry: Registry,
}

impl CredsServer {
	/// Bind a loopback server on an ephemeral port and serve in the background.
	pub async fn start() -> Result<Self> {
		let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
		let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
			.await
			.into_diagnostic()
			.wrap_err("binding loopback container-credentials endpoint")?;
		let addr: SocketAddr = listener
			.local_addr()
			.into_diagnostic()
			.wrap_err("reading container-credentials endpoint address")?;
		// Loopback literal (not "localhost"): minio-go rejects a FULL_URI whose
		// host resolves to a non-loopback address.
		let base_uri = format!("http://127.0.0.1:{}", addr.port());

		let app = Router::new()
			.route("/creds", get(handler))
			.with_state(registry.clone());
		tokio::spawn(async move {
			if let Err(err) = axum::serve(listener, app).await {
				error!("container-credentials endpoint exited: {err}");
			}
		});

		Ok(CredsServer { base_uri, registry })
	}

	/// Lease a token for the lifetime of the returned [`CredsLease`]. `refresh`
	/// is called to obtain (and later re-obtain) creds.
	pub fn lease(&self, refresh: Refresher) -> CredsLease {
		let token = Uuid::new_v4().to_string();
		self.registry.lock().unwrap().insert(
			token.clone(),
			Arc::new(Lease {
				refresh,
				cached: AsyncMutex::new(None),
			}),
		);
		CredsLease {
			uri: format!("{}/creds", self.base_uri),
			token,
			registry: self.registry.clone(),
		}
	}
}

/// An active credentials lease. Deregisters its token on drop.
pub struct CredsLease {
	uri: String,
	token: String,
	registry: Registry,
}

impl CredsLease {
	/// The `AWS_CONTAINER_CREDENTIALS_FULL_URI` to set on the kopia subprocess.
	pub fn uri(&self) -> &str {
		&self.uri
	}

	/// The `AWS_CONTAINER_AUTHORIZATION_TOKEN` to set on the kopia subprocess.
	pub fn token(&self) -> &str {
		&self.token
	}
}

impl Drop for CredsLease {
	fn drop(&mut self) {
		if let Ok(mut reg) = self.registry.lock() {
			reg.remove(&self.token);
		}
	}
}

/// Whether cached creds are absent or within the refresh margin of expiry.
fn needs_refresh(cached: &Option<BackupCredentials>, now: Timestamp) -> bool {
	match cached {
		None => true,
		Some(creds) => creds.expiration <= now + REFRESH_MARGIN_MINUTES.minutes(),
	}
}

async fn handler(
	State(registry): State<Registry>,
	headers: HeaderMap,
) -> std::result::Result<Json<ContainerCreds>, StatusCode> {
	let token = headers
		.get(AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		.unwrap_or_default();
	// Clone the Arc out of the std mutex; never hold it across an await.
	let lease = registry.lock().unwrap().get(token).cloned();
	let Some(lease) = lease else {
		return Err(StatusCode::FORBIDDEN);
	};

	let mut cached = lease.cached.lock().await;
	if needs_refresh(&cached, Timestamp::now()) {
		let fresh = (lease.refresh)().await.map_err(|err| {
			error!("refreshing backup credentials failed: {err}");
			StatusCode::BAD_GATEWAY
		})?;
		*cached = Some(fresh);
	}
	let creds = cached.as_ref().expect("cache populated above");
	Ok(Json(ContainerCreds::from(creds)))
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicUsize, Ordering};

	use serde_json::json;

	use super::*;

	fn creds_expiring(expiration: &str) -> BackupCredentials {
		serde_json::from_value(json!({
			"Version": 1,
			"AccessKeyId": "AKIA",
			"SecretAccessKey": "secret",
			"SessionToken": "session",
			"Expiration": expiration,
		}))
		.unwrap()
	}

	fn registry_with(token: &str, lease: Lease) -> Registry {
		let mut map = HashMap::new();
		map.insert(token.to_owned(), Arc::new(lease));
		Arc::new(Mutex::new(map))
	}

	fn header(token: &str) -> HeaderMap {
		let mut h = HeaderMap::new();
		h.insert(AUTHORIZATION, token.parse().unwrap());
		h
	}

	#[tokio::test]
	async fn known_token_returns_cached_creds_without_refreshing() {
		// Expiry far in the future, so no refresh; refresher would panic if called.
		let lease = Lease {
			refresh: Arc::new(|| Box::pin(async { panic!("must not refresh") })),
			cached: AsyncMutex::new(Some(creds_expiring("2099-01-01T00:00:00Z"))),
		};
		let registry = registry_with("good-token", lease);
		let Json(out) = handler(State(registry), header("good-token"))
			.await
			.expect("known token serves creds");
		let value = serde_json::to_value(&out).unwrap();
		assert_eq!(value["AccessKeyId"], "AKIA");
		assert_eq!(value["Token"], "session");
		assert!(value.get("SessionToken").is_none());
	}

	#[tokio::test]
	async fn unknown_token_is_forbidden() {
		let lease = Lease {
			refresh: Arc::new(|| Box::pin(async { panic!("must not refresh") })),
			cached: AsyncMutex::new(Some(creds_expiring("2099-01-01T00:00:00Z"))),
		};
		let registry = registry_with("good-token", lease);
		let err = handler(State(registry), header("wrong-token"))
			.await
			.expect_err("unknown token rejected");
		assert_eq!(err, StatusCode::FORBIDDEN);
	}

	#[tokio::test]
	async fn empty_cache_triggers_a_refresh() {
		let calls = Arc::new(AtomicUsize::new(0));
		let calls2 = calls.clone();
		let lease = Lease {
			refresh: Arc::new(move || {
				calls2.fetch_add(1, Ordering::SeqCst);
				Box::pin(async { Ok(creds_expiring("2099-01-01T00:00:00Z")) })
			}),
			cached: AsyncMutex::new(None),
		};
		let registry = registry_with("good-token", lease);
		let Json(out) = handler(State(registry), header("good-token"))
			.await
			.expect("refresh populates the cache");
		assert_eq!(calls.load(Ordering::SeqCst), 1);
		assert_eq!(serde_json::to_value(&out).unwrap()["Token"], "session");
	}

	#[test]
	fn needs_refresh_logic() {
		let now: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
		assert!(needs_refresh(&None, now));
		// Within the 2-minute margin → refresh.
		assert!(needs_refresh(
			&Some(creds_expiring("2026-01-01T00:01:00Z")),
			now
		));
		// Comfortably ahead → no refresh.
		assert!(!needs_refresh(
			&Some(creds_expiring("2026-01-01T01:00:00Z")),
			now
		));
	}
}
