//! Wire types for Canopy's backup-credentials endpoints.
//!
//! These mirror the canopy public-server contract (`CapabilitiesArgs`,
//! `CredentialsArgs`, `ReportArgs`, `CredentialProcessOutput`, `BackupTarget`).
//! The device fetches short-lived S3 creds and the repo target from Canopy on
//! each run, then serves the creds to kopia's minio-go provider in the
//! [`ContainerCreds`] shape (note `Token`, not `SessionToken`).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Redacted;

/// Why a credential was issued / a run executed.
///
/// A real capability gate on the issued S3 creds: `backup` grants
/// write-without-delete, `restore` grants read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
	#[default]
	Backup,
	Restore,
}

/// Outcome of a reported backup/restore run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
	Success,
	Failure,
}

/// Body for `POST /backup-capabilities`: the backup types this server can run.
#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesRequest<'a> {
	pub types: &'a [String],
}

/// Body for `POST /backup-credentials`.
#[derive(Debug, Clone, Serialize)]
pub struct BackupCredentialsRequest<'a> {
	pub r#type: &'a str,
	pub purpose: Purpose,
}

/// `credential_process`-shaped creds returned by `POST /backup-credentials`.
///
/// Field names are fixed by the AWS SDK (PascalCase). The driver translates
/// these into the [`ContainerCreds`] shape kopia's minio-go provider polls for.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BackupCredentials {
	pub version: u32,
	pub access_key_id: String,
	pub secret_access_key: Redacted<String>,
	pub session_token: Redacted<String>,
	pub expiration: Timestamp,
}

/// Creds in the ECS container-credentials shape kopia's minio-go provider polls
/// for: note **`Token`** (not `SessionToken`), and `Expiration` as RFC3339 `Z`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerCreds {
	pub access_key_id: String,
	pub secret_access_key: Redacted<String>,
	pub token: Redacted<String>,
	pub expiration: Timestamp,
}

impl From<&BackupCredentials> for ContainerCreds {
	fn from(c: &BackupCredentials) -> Self {
		Self {
			access_key_id: c.access_key_id.clone(),
			secret_access_key: c.secret_access_key.clone(),
			token: c.session_token.clone(),
			expiration: c.expiration,
		}
	}
}

/// The S3 repo target returned by `GET /backup-target`.
#[derive(Debug, Clone, Deserialize)]
pub struct BackupTarget {
	/// Always `"s3"`.
	pub storage: String,
	pub bucket: String,
	/// Normally empty (the repo lives at the bucket root).
	#[serde(default)]
	pub prefix: String,
	pub region: String,
	pub repo_password: Redacted<String>,
}

/// Result of `GET /backup-target`: a live target, or the benign dormant state
/// (the device is not yet authorised for backups — `412`/`409`).
#[derive(Debug, Clone)]
pub enum TargetOutcome {
	Ready(BackupTarget),
	Dormant,
}

/// Body for `POST /backup-report`.
#[derive(Debug, Clone, Serialize)]
pub struct BackupReport<'a> {
	/// The run-uuid bestool minted at run start (becomes `backup_runs.id`).
	pub run_id: &'a str,
	pub r#type: &'a str,
	pub purpose: Purpose,
	pub outcome: Outcome,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub error: Option<&'a str>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub bytes_uploaded: Option<i64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub snapshot_id: Option<&'a str>,
	/// S3 traffic the run accounted for, measured by the re-signing proxy. `*_raw`
	/// is the full HTTP message (headers + on-the-wire body incl. SigV4 chunk
	/// framing); `*_payload` is the decoded object data. A rough per-deployment
	/// network/S3 measure; distinct from `bytes_uploaded` (kopia's own figure).
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
	fn purpose_serialises_lowercase() {
		assert_eq!(
			serde_json::to_value(Purpose::Backup).unwrap(),
			json!("backup")
		);
		assert_eq!(
			serde_json::to_value(Purpose::Restore).unwrap(),
			json!("restore")
		);
	}

	#[test]
	fn purpose_defaults_to_backup() {
		assert_eq!(Purpose::default(), Purpose::Backup);
	}

	#[test]
	fn outcome_serialises_lowercase() {
		assert_eq!(
			serde_json::to_value(Outcome::Success).unwrap(),
			json!("success")
		);
		assert_eq!(
			serde_json::to_value(Outcome::Failure).unwrap(),
			json!("failure")
		);
	}

	#[test]
	fn credentials_request_carries_type_and_purpose() {
		let req = BackupCredentialsRequest {
			r#type: "tamanu-postgres",
			purpose: Purpose::Backup,
		};
		assert_eq!(
			serde_json::to_value(&req).unwrap(),
			json!({"type": "tamanu-postgres", "purpose": "backup"})
		);
	}

	#[test]
	fn capabilities_request_lists_types() {
		let types = vec!["tamanu-postgres".to_owned(), "files".to_owned()];
		let req = CapabilitiesRequest { types: &types };
		assert_eq!(
			serde_json::to_value(&req).unwrap(),
			json!({"types": ["tamanu-postgres", "files"]})
		);
	}

	#[test]
	fn backup_credentials_deserialise_from_credential_process_shape() {
		let body = json!({
			"Version": 1,
			"AccessKeyId": "AKIA",
			"SecretAccessKey": "secret",
			"SessionToken": "token",
			"Expiration": "2026-05-21T13:00:00Z",
		});
		let creds: BackupCredentials = serde_json::from_value(body).unwrap();
		assert_eq!(creds.version, 1);
		assert_eq!(creds.access_key_id, "AKIA");
		assert_eq!(&*creds.secret_access_key, "secret");
		assert_eq!(&*creds.session_token, "token");
		assert_eq!(creds.expiration.to_string(), "2026-05-21T13:00:00Z");
	}

	#[test]
	fn container_creds_translate_session_token_to_token() {
		let body = json!({
			"Version": 1,
			"AccessKeyId": "AKIA",
			"SecretAccessKey": "secret",
			"SessionToken": "session-token",
			"Expiration": "2026-05-21T13:00:00Z",
		});
		let creds: BackupCredentials = serde_json::from_value(body).unwrap();
		let container = ContainerCreds::from(&creds);
		let out = serde_json::to_value(&container).unwrap();
		assert_eq!(
			out,
			json!({
				"AccessKeyId": "AKIA",
				"SecretAccessKey": "secret",
				"Token": "session-token",
				"Expiration": "2026-05-21T13:00:00Z",
			})
		);
		// No SessionToken key leaks through.
		assert!(out.get("SessionToken").is_none());
	}

	#[test]
	fn backup_target_deserialises() {
		let body = json!({
			"storage": "s3",
			"bucket": "my-bucket",
			"prefix": "",
			"region": "ap-southeast-2",
			"repo_password": "hunter2",
		});
		let target: BackupTarget = serde_json::from_value(body).unwrap();
		assert_eq!(target.storage, "s3");
		assert_eq!(target.bucket, "my-bucket");
		assert_eq!(target.prefix, "");
		assert_eq!(target.region, "ap-southeast-2");
		assert_eq!(&*target.repo_password, "hunter2");
	}

	#[test]
	fn backup_report_omits_optional_fields() {
		let report = BackupReport {
			run_id: "11111111-1111-1111-1111-111111111111",
			r#type: "tamanu-postgres",
			purpose: Purpose::Backup,
			outcome: Outcome::Success,
			error: None,
			bytes_uploaded: None,
			snapshot_id: None,
			s3_sent_raw_bytes: None,
			s3_sent_payload_bytes: None,
			s3_received_raw_bytes: None,
			s3_received_payload_bytes: None,
		};
		assert_eq!(
			serde_json::to_value(&report).unwrap(),
			json!({
				"run_id": "11111111-1111-1111-1111-111111111111",
				"type": "tamanu-postgres",
				"purpose": "backup",
				"outcome": "success",
			})
		);
	}

	#[test]
	fn backup_report_includes_failure_fields() {
		let report = BackupReport {
			run_id: "run",
			r#type: "tamanu-postgres",
			purpose: Purpose::Backup,
			outcome: Outcome::Failure,
			error: Some("kopia exploded"),
			bytes_uploaded: Some(42),
			snapshot_id: Some("snap"),
			s3_sent_raw_bytes: Some(2048),
			s3_sent_payload_bytes: Some(1024),
			s3_received_raw_bytes: Some(64),
			s3_received_payload_bytes: Some(0),
		};
		assert_eq!(
			serde_json::to_value(&report).unwrap(),
			json!({
				"run_id": "run",
				"type": "tamanu-postgres",
				"purpose": "backup",
				"outcome": "failure",
				"error": "kopia exploded",
				"bytes_uploaded": 42,
				"snapshot_id": "snap",
				"s3_sent_raw_bytes": 2048,
				"s3_sent_payload_bytes": 1024,
				"s3_received_raw_bytes": 64,
				"s3_received_payload_bytes": 0,
			})
		);
	}

	#[test]
	fn redacted_debug_does_not_leak() {
		let creds = ContainerCreds {
			access_key_id: "AKIA".to_owned(),
			secret_access_key: Redacted("aws-sk-value-123".to_owned()),
			token: Redacted("aws-token-value-456".to_owned()),
			expiration: "2026-05-21T13:00:00Z".parse().unwrap(),
		};
		let debug = format!("{creds:?}");
		assert!(!debug.contains("aws-sk-value-123"));
		assert!(!debug.contains("aws-token-value-456"));
		assert!(debug.contains("<redacted>"));
	}
}
