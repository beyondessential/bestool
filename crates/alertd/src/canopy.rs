use std::time::Duration;

use jiff::Timestamp;
use miette::{IntoDiagnostic, Result, WrapErr};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use time::{Duration as TimeDuration, OffsetDateTime};
use tracing::debug;

pub const DEFAULT_CANOPY_URL: &str = "https://meta.tamanu.app";

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
#[derive(Debug, Clone)]
pub struct CanopyClient {
	http: reqwest::Client,
}

impl CanopyClient {
	/// Build a canopy client from a Tamanu device key (PKCS8 PEM).
	///
	/// Generates a fresh self-signed certificate from the key with ~30 days validity,
	/// then constructs a reqwest client whose TLS identity is that certificate plus
	/// the private key. The canopy edge expects this certificate via mTLS; it
	/// authenticates the request by looking up the public key in its device registry.
	pub fn new(device_key_pem: &str) -> Result<Self> {
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
		params.not_after = now + TimeDuration::days(30);

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

		let http = reqwest::Client::builder()
			.identity(identity)
			.use_rustls_tls()
			.timeout(Duration::from_secs(30))
			.build()
			.into_diagnostic()
			.wrap_err("building canopy HTTP client")?;

		Ok(Self { http })
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

		let response = self
			.http
			.post(url)
			.json(&event)
			.send()
			.await
			.into_diagnostic()
			.wrap_err("posting event to canopy")?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			return Err(miette::miette!(
				"canopy /events returned {status}: {body}"
			));
		}

		Ok(())
	}
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
