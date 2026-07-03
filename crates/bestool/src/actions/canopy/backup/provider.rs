//! Credential provider for the S3 re-signing proxy, backed by Canopy.
//!
//! Fetches STS credentials from Canopy's `/backup-credentials` endpoint for a
//! `(type, purpose)`, caches them, and refreshes as they near expiry. The proxy
//! asks for credentials once per request; this answers from cache while they're
//! valid and re-fetches when they aren't, so a kopia run outlives any single
//! issuance.

use std::{future::Future, pin::Pin, sync::Arc};

use bestool_canopy::{
	CanopyClient,
	schema::{BackupPurpose, CredentialProcessOutput},
};
use bestool_kopia::proxy::{BoxError, CredentialProvider, Credentials};
use futures::future::BoxFuture;
use jiff::{Timestamp, ToSpan};
use reqwest::Url;
use tokio::sync::Mutex;

/// Refresh creds this far ahead of their `Expiration`.
const REFRESH_MARGIN_MINUTES: i64 = 2;

/// Fetches a fresh set of creds from Canopy. Boxed so the provider is testable
/// without a live `CanopyClient`.
type Refresher =
	Arc<dyn Fn() -> BoxFuture<'static, Result<CredentialProcessOutput, String>> + Send + Sync>;

/// A [`CredentialProvider`] that draws backup credentials from Canopy.
pub struct CanopyCredentialProvider {
	refresh: Refresher,
	cached: Mutex<Option<CredentialProcessOutput>>,
}

impl CanopyCredentialProvider {
	/// Build a provider that fetches `(backup_type, purpose)` credentials from
	/// Canopy.
	pub fn new(
		client: Arc<CanopyClient>,
		base_url: Url,
		backup_type: String,
		purpose: BackupPurpose,
	) -> Self {
		let refresh: Refresher = Arc::new(move || {
			let client = client.clone();
			let base_url = base_url.clone();
			let backup_type = backup_type.clone();
			Box::pin(async move {
				client
					.backup_credentials(&base_url, &backup_type, purpose)
					.await
					.map_err(|err| format!("{err}"))
			})
		});
		Self::with_refresher(refresh)
	}

	fn with_refresher(refresh: Refresher) -> Self {
		Self {
			refresh,
			cached: Mutex::new(None),
		}
	}
}

/// Whether cached creds are absent or within the refresh margin of expiry.
fn needs_refresh(cached: &Option<CredentialProcessOutput>, now: Timestamp) -> bool {
	match cached {
		None => true,
		Some(creds) => creds.expiration <= now + REFRESH_MARGIN_MINUTES.minutes(),
	}
}

impl CredentialProvider for CanopyCredentialProvider {
	fn credentials(
		&self,
	) -> Pin<Box<dyn Future<Output = Result<Credentials, BoxError>> + Send + '_>> {
		Box::pin(async move {
			let mut cached = self.cached.lock().await;
			if needs_refresh(&cached, Timestamp::now()) {
				let fresh = (self.refresh)().await.map_err(|err| -> BoxError { err.into() })?;
				*cached = Some(fresh);
			}
			let creds = cached.as_ref().expect("cache populated above");
			Ok(Credentials {
				access_key: creds.access_key_id.clone(),
				secret_key: creds.secret_access_key.0.clone(),
				session_token: Some(creds.session_token.0.clone()),
			})
		})
	}
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicUsize, Ordering};

	use serde_json::json;

	use super::*;

	fn creds_expiring(expiration: &str) -> CredentialProcessOutput {
		serde_json::from_value(json!({
			"Version": 1,
			"AccessKeyId": "AKIA",
			"SecretAccessKey": "secret",
			"SessionToken": "session",
			"Expiration": expiration,
		}))
		.unwrap()
	}

	#[tokio::test]
	async fn maps_fields_and_caches_without_refetching() {
		let calls = Arc::new(AtomicUsize::new(0));
		let calls2 = calls.clone();
		let provider = CanopyCredentialProvider::with_refresher(Arc::new(move || {
			calls2.fetch_add(1, Ordering::SeqCst);
			Box::pin(async { Ok(creds_expiring("2099-01-01T00:00:00Z")) })
		}));

		let first = provider.credentials().await.unwrap();
		assert_eq!(first.access_key, "AKIA");
		assert_eq!(first.secret_key, "secret");
		assert_eq!(first.session_token.as_deref(), Some("session"));

		// Still valid: second call serves the cache, no re-fetch.
		provider.credentials().await.unwrap();
		assert_eq!(calls.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn needs_refresh_logic() {
		let now: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
		assert!(needs_refresh(&None, now));
		assert!(needs_refresh(
			&Some(creds_expiring("2026-01-01T00:01:00Z")),
			now
		));
		assert!(!needs_refresh(
			&Some(creds_expiring("2026-01-01T01:00:00Z")),
			now
		));
	}
}
