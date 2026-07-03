//! Backup-specific types that aren't part of canopy's wire contract.
//!
//! The request and response bodies for the backup endpoints are generated from
//! canopy's OpenAPI document — see the [`schema`](crate::schema) module. This
//! module holds the two shapes that don't map onto a schema type: the container
//! credentials kopia's minio-go provider polls for, and the local result of a
//! [`GET /backup-target`](crate::CanopyClient::backup_target) that folds the
//! dormant device state into an enum.

use jiff::Timestamp;
use miette::Result;
use reqwest::StatusCode;
use serde::Serialize;

use crate::{
	Redacted,
	client::CanopyHttpError,
	schema::{BackupTarget, CredentialProcessOutput},
};

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

impl From<&CredentialProcessOutput> for ContainerCreds {
	fn from(c: &CredentialProcessOutput) -> Self {
		Self {
			access_key_id: c.access_key_id.clone(),
			secret_access_key: c.secret_access_key.clone(),
			token: c.session_token.clone(),
			expiration: c.expiration,
		}
	}
}

/// Result of [`GET /backup-target`](crate::CanopyClient::backup_target): a live
/// target, or the benign dormant state (the device is not yet authorised for
/// backups — `412`/`409`).
#[derive(Debug, Clone)]
pub enum TargetOutcome {
	Ready(BackupTarget),
	Dormant,
}

impl TargetOutcome {
	/// Interpret a [`backup_target`](crate::CanopyClient::backup_target) result:
	/// a `412`/`409` (the device isn't yet authorised for backups) becomes
	/// [`Dormant`](Self::Dormant), a target becomes [`Ready`](Self::Ready), and
	/// any other error propagates.
	pub fn from_result(result: Result<BackupTarget>) -> Result<Self> {
		match result {
			Ok(target) => Ok(Self::Ready(target)),
			Err(report) => match report.downcast_ref::<CanopyHttpError>() {
				Some(err)
					if err.status == StatusCode::PRECONDITION_FAILED
						|| err.status == StatusCode::CONFLICT =>
				{
					Ok(Self::Dormant)
				}
				_ => Err(report),
			},
		}
	}
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	#[test]
	fn container_creds_translate_session_token_to_token() {
		let creds: CredentialProcessOutput = serde_json::from_value(json!({
			"Version": 1,
			"AccessKeyId": "AKIA",
			"SecretAccessKey": "secret",
			"SessionToken": "session-token",
			"Expiration": "2026-05-21T13:00:00Z",
		}))
		.unwrap();
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
