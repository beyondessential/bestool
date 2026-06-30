//! Wire types for Canopy's managed-restore endpoints (the PGRO restore consumer).
//!
//! Canopy drives the consumer via a worklist. The consumer registers the restore
//! intents it supports, fetches the worklist of concrete replicas to restore,
//! pulls per-group read-only credentials and the repo password, then reports each
//! restore's health back. These types mirror the frozen canopy public-server
//! contract; field renaming via serde matches the JSON.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{BackupCredentials, Outcome, Redacted};

/// Body for `POST /restore-capabilities`: the restore intents this consumer supports.
///
/// Replaces the registered intent set wholesale; canopy dispatches only matching
/// worklist entries.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreCapabilitiesRequest<'a> {
	pub intents: &'a [&'a str],
}

/// One entry of `GET /restore-worklist`: a concrete replica to restore.
///
/// Declarations are expanded per live server, capability-filtered, and
/// server-specific-over-group-wide deduped, so there is one entry per replica.
/// `intent` is an open set passed through to the consumer.
#[derive(Debug, Clone, Deserialize)]
pub struct WorklistEntry {
	pub replica_id: Uuid,
	pub group_id: Uuid,
	pub server_id: Uuid,
	pub r#type: String,
	pub intent: String,
	/// Operator label.
	pub name: String,
	/// Maximum acceptable age of the restored snapshot; `None` means always latest.
	pub freshness_seconds: Option<i64>,
	/// Kopia snapshot id to restore; `None` when the group has no successful backup yet.
	pub snapshot_id: Option<String>,
	/// RFC3339 timestamp of `snapshot_id`; `None` alongside it.
	pub snapshot_at: Option<String>,
	/// Always `"s3"`.
	pub storage: String,
	pub bucket: String,
	pub prefix: String,
	pub region: String,
}

/// Body for `POST /restore-credentials`.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreCredentialsRequest<'a> {
	pub group: Uuid,
	pub r#type: &'a str,
}

/// Read-only S3 credentials plus the repo password, from `POST /restore-credentials`.
///
/// `credentials` is the `credential_process` shape the proxy re-signs onto kopia;
/// `repo_password` opens the kopia repo. One per-group call fully opens the repo.
#[derive(Debug, Clone, Deserialize)]
pub struct RestoreCredentials {
	pub credentials: BackupCredentials,
	pub repo_password: Redacted<String>,
}

/// Body for `POST /restore-verification`: a restore's reported health.
///
/// Report `outcome = Success` with `replica_healthy = true` only when the
/// deployment passed the readiness gate; anything else raises a group-level
/// incident. Unsupported intents are handled by capability registration, never
/// reported here.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreVerification<'a> {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub replica_id: Option<Uuid>,
	pub group: Uuid,
	pub server_id: Uuid,
	pub r#type: &'a str,
	pub intent: &'a str,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub snapshot_id: Option<&'a str>,
	pub outcome: Outcome,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub error: Option<&'a str>,
	pub replica_healthy: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub postgres_version: Option<&'a str>,
	pub observed_at: Timestamp,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub s3_sent_raw_bytes: Option<i64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub s3_sent_payload_bytes: Option<i64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub s3_received_raw_bytes: Option<i64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub s3_received_payload_bytes: Option<i64>,
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	#[test]
	fn capabilities_request_lists_intents() {
		let intents = ["verify", "analytics", "disaster-recovery"];
		let req = RestoreCapabilitiesRequest { intents: &intents };
		assert_eq!(
			serde_json::to_value(&req).unwrap(),
			json!({"intents": ["verify", "analytics", "disaster-recovery"]})
		);
	}

	#[test]
	fn credentials_request_carries_group_and_type() {
		let group = "11111111-1111-1111-1111-111111111111".parse().unwrap();
		let req = RestoreCredentialsRequest {
			group,
			r#type: "tamanu-postgres",
		};
		assert_eq!(
			serde_json::to_value(&req).unwrap(),
			json!({
				"group": "11111111-1111-1111-1111-111111111111",
				"type": "tamanu-postgres",
			})
		);
	}

	#[test]
	fn worklist_entry_deserialises() {
		let body = json!({
			"replica_id": "11111111-1111-1111-1111-111111111111",
			"group_id": "22222222-2222-2222-2222-222222222222",
			"server_id": "33333333-3333-3333-3333-333333333333",
			"type": "tamanu-postgres",
			"intent": "verify",
			"name": "nightly verify",
			"freshness_seconds": 86400,
			"snapshot_id": "kopia-snap",
			"snapshot_at": "2026-06-30T00:00:00Z",
			"storage": "s3",
			"bucket": "my-bucket",
			"prefix": "",
			"region": "ap-southeast-2",
		});
		let entry: WorklistEntry = serde_json::from_value(body).unwrap();
		assert_eq!(entry.r#type, "tamanu-postgres");
		assert_eq!(entry.intent, "verify");
		assert_eq!(entry.name, "nightly verify");
		assert_eq!(entry.freshness_seconds, Some(86400));
		assert_eq!(entry.snapshot_id.as_deref(), Some("kopia-snap"));
		assert_eq!(entry.bucket, "my-bucket");
	}

	#[test]
	fn worklist_entry_allows_null_snapshot_and_freshness() {
		let body = json!({
			"replica_id": "11111111-1111-1111-1111-111111111111",
			"group_id": "22222222-2222-2222-2222-222222222222",
			"server_id": "33333333-3333-3333-3333-333333333333",
			"type": "tamanu-postgres",
			"intent": "verify",
			"name": "latest",
			"freshness_seconds": null,
			"snapshot_id": null,
			"snapshot_at": null,
			"storage": "s3",
			"bucket": "my-bucket",
			"prefix": "",
			"region": "ap-southeast-2",
		});
		let entry: WorklistEntry = serde_json::from_value(body).unwrap();
		assert_eq!(entry.freshness_seconds, None);
		assert_eq!(entry.snapshot_id, None);
		assert_eq!(entry.snapshot_at, None);
	}

	#[test]
	fn restore_credentials_deserialise_composite() {
		let body = json!({
			"credentials": {
				"Version": 1,
				"AccessKeyId": "AKIA",
				"SecretAccessKey": "secret",
				"SessionToken": "token",
				"Expiration": "2026-05-21T13:00:00Z",
			},
			"repo_password": "hunter2",
		});
		let creds: RestoreCredentials = serde_json::from_value(body).unwrap();
		assert_eq!(creds.credentials.access_key_id, "AKIA");
		assert_eq!(&*creds.repo_password, "hunter2");
	}

	#[test]
	fn restore_credentials_debug_does_not_leak_password() {
		let body = json!({
			"credentials": {
				"Version": 1,
				"AccessKeyId": "AKIA",
				"SecretAccessKey": "secret",
				"SessionToken": "token",
				"Expiration": "2026-05-21T13:00:00Z",
			},
			"repo_password": "super-secret-repo-pw",
		});
		let creds: RestoreCredentials = serde_json::from_value(body).unwrap();
		let debug = format!("{creds:?}");
		assert!(!debug.contains("super-secret-repo-pw"));
		assert!(debug.contains("<redacted>"));
	}

	#[test]
	fn verification_omits_optional_fields() {
		let report = RestoreVerification {
			replica_id: None,
			group: "22222222-2222-2222-2222-222222222222".parse().unwrap(),
			server_id: "33333333-3333-3333-3333-333333333333".parse().unwrap(),
			r#type: "tamanu-postgres",
			intent: "verify",
			snapshot_id: None,
			outcome: Outcome::Failure,
			error: None,
			replica_healthy: false,
			postgres_version: None,
			observed_at: "2026-06-30T00:00:00Z".parse().unwrap(),
			s3_sent_raw_bytes: None,
			s3_sent_payload_bytes: None,
			s3_received_raw_bytes: None,
			s3_received_payload_bytes: None,
		};
		assert_eq!(
			serde_json::to_value(&report).unwrap(),
			json!({
				"group": "22222222-2222-2222-2222-222222222222",
				"server_id": "33333333-3333-3333-3333-333333333333",
				"type": "tamanu-postgres",
				"intent": "verify",
				"outcome": "failure",
				"replica_healthy": false,
				"observed_at": "2026-06-30T00:00:00Z",
			})
		);
	}

	#[test]
	fn verification_includes_full_payload() {
		let report = RestoreVerification {
			replica_id: Some("11111111-1111-1111-1111-111111111111".parse().unwrap()),
			group: "22222222-2222-2222-2222-222222222222".parse().unwrap(),
			server_id: "33333333-3333-3333-3333-333333333333".parse().unwrap(),
			r#type: "tamanu-postgres",
			intent: "verify",
			snapshot_id: Some("kopia-snap"),
			outcome: Outcome::Success,
			error: None,
			replica_healthy: true,
			postgres_version: Some("15"),
			observed_at: "2026-06-30T00:00:00Z".parse().unwrap(),
			s3_sent_raw_bytes: Some(123),
			s3_sent_payload_bytes: Some(120),
			s3_received_raw_bytes: Some(456),
			s3_received_payload_bytes: Some(450),
		};
		assert_eq!(
			serde_json::to_value(&report).unwrap(),
			json!({
				"replica_id": "11111111-1111-1111-1111-111111111111",
				"group": "22222222-2222-2222-2222-222222222222",
				"server_id": "33333333-3333-3333-3333-333333333333",
				"type": "tamanu-postgres",
				"intent": "verify",
				"snapshot_id": "kopia-snap",
				"outcome": "success",
				"replica_healthy": true,
				"postgres_version": "15",
				"observed_at": "2026-06-30T00:00:00Z",
				"s3_sent_raw_bytes": 123,
				"s3_sent_payload_bytes": 120,
				"s3_received_raw_bytes": 456,
				"s3_received_payload_bytes": 450,
			})
		);
	}
}
