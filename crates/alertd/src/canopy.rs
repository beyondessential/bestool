use std::{fmt, time::Duration};

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

/// HTTP client with mTLS configured for talking to a canopy server.
///
/// The TLS identity is a fresh self-signed cert generated from the device key
/// at construction. The cert is short-lived ([`CERT_VALIDITY_DAYS`]); for long-
/// running daemons, [`Self::renew`] should be called on a timer
/// ([`CERT_RENEW_AFTER`]) to swap in a fresh cert+client before expiry.
pub struct CanopyClient {
	device_key: Redacted<String>,
	http: RwLock<reqwest::Client>,
}

impl fmt::Debug for CanopyClient {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CanopyClient").finish_non_exhaustive()
	}
}

impl CanopyClient {
	/// Build a canopy client from a Tamanu device key (PKCS8 PEM).
	///
	/// Generates a fresh self-signed cert from the key (validity
	/// [`CERT_VALIDITY_DAYS`]) and wraps it in a `reqwest::Client` whose TLS
	/// identity is that cert plus the private key. The canopy edge expects
	/// this cert via mTLS and authenticates the request by looking the public
	/// key up in its device registry.
	pub fn new(device_key_pem: &str) -> Result<Self> {
		let http = build_http(device_key_pem)?;
		Ok(Self {
			device_key: Redacted(device_key_pem.to_owned()),
			http: RwLock::new(http),
		})
	}

	/// Rebuild the underlying HTTP client with a fresh certificate.
	///
	/// Atomically replaces the live client; in-flight requests continue with
	/// the old client until they complete. Call this on a timer well inside
	/// the cert validity window.
	pub async fn renew(&self) -> Result<()> {
		let http = build_http(&self.device_key.0)?;
		*self.http.write().await = http;
		Ok(())
	}

	/// POST an event to `{base_url}/events`.
	pub async fn post_event(&self, base_url: &Url, event: NewEvent<'_>) -> Result<()> {
		let url = base_url
			.join("/events")
			.into_diagnostic()
			.wrap_err("building /events URL")?;

		debug!(
			%url,
			source = event.source,
			r#ref = event.r#ref,
			active = ?event.active,
			"posting event to canopy"
		);

		let http = self.http.read().await.clone();
		let response = http
			.post(url)
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

fn build_http(device_key_pem: &str) -> Result<reqwest::Client> {
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
	fn build_client_from_p256_key() {
		let client = CanopyClient::new(TEST_DEVICE_KEY);
		assert!(client.is_ok(), "{:?}", client.err());
	}

	#[tokio::test]
	async fn renew_swaps_in_fresh_client() {
		let client = CanopyClient::new(TEST_DEVICE_KEY).unwrap();
		client.renew().await.expect("renew should succeed");
	}

	#[test]
	fn build_client_fails_on_garbage_key() {
		assert!(CanopyClient::new("not a real PEM").is_err());
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
