//! Proves the build-time generated `schema` layer exists, has the expected
//! shape, and round-trips through serde. These run against the committed
//! snapshot: generation already happened at build time, so no network is
//! needed here.

use bestool_canopy::schema::{
	BackupPurpose, BackupTarget, BeginArgs, BeginResponse, CompleteArgs, CompleteResponse,
	CredentialProcessOutput, ReportArgs, RunOutcome,
};

#[test]
fn backup_target_response_roundtrips() {
	let json = serde_json::json!({
		"storage": "s3",
		"bucket": "backups",
		"prefix": "",
		"region": "ap-southeast-2",
		"repo_password": "hunter2",
	});
	let target: BackupTarget = serde_json::from_value(json.clone()).unwrap();
	assert_eq!(target.storage, "s3");
	assert_eq!(target.bucket, "backups");
	assert_eq!(target.region, "ap-southeast-2");
	assert_eq!(serde_json::to_value(&target).unwrap(), json);
}

#[test]
fn report_args_request_roundtrips() {
	let json = serde_json::json!({
		"run_id": "6f8a2b1c-0000-4000-8000-000000000000",
		"type": "postgres",
		"purpose": "backup",
		"outcome": "success",
		"bytes_uploaded": 1234,
	});
	let report: ReportArgs = serde_json::from_value(json).unwrap();
	assert_eq!(report.type_, "postgres");
	assert_eq!(report.purpose, BackupPurpose::Backup);
	assert_eq!(report.outcome, RunOutcome::Success);
	assert_eq!(report.bytes_uploaded, Some(1234));

	let back = serde_json::to_value(&report).unwrap();
	assert_eq!(back["type"], "postgres");
	assert_eq!(back["outcome"], "success");
}

#[test]
fn register_begin_types_roundtrip() {
	let args_json = serde_json::json!({
		"server_id": "6f8a2b1c-0000-4000-8000-000000000000",
		"token": "enrol-token",
	});
	let args: BeginArgs = serde_json::from_value(args_json).unwrap();
	assert_eq!(args.token, "enrol-token");
	assert!(args.spki.is_none());

	let resp: BeginResponse = serde_json::from_value(serde_json::json!({
		"nonce": "YmFzZTY0",
		"channel_binding_required": true,
	}))
	.unwrap();
	assert!(resp.channel_binding_required);
	assert_eq!(resp.nonce, "YmFzZTY0");
}

#[test]
fn register_complete_types_roundtrip() {
	let args: CompleteArgs = serde_json::from_value(serde_json::json!({
		"server_id": "6f8a2b1c-0000-4000-8000-000000000000",
		"nonce": "YmFzZTY0",
		"signature": "c2ln",
	}))
	.unwrap();
	assert_eq!(args.signature, "c2ln");

	let resp: CompleteResponse = serde_json::from_value(serde_json::json!({
		"server_id": "6f8a2b1c-0000-4000-8000-000000000000",
		"device_id": "1111aaaa-0000-4000-8000-000000000000",
	}))
	.unwrap();
	assert_eq!(
		resp.device_id.to_string(),
		"1111aaaa-0000-4000-8000-000000000000"
	);
}

#[test]
fn credential_secrets_are_redacted_and_expiry_is_a_timestamp() {
	let wire = serde_json::json!({
		"Version": 1,
		"AccessKeyId": "AKIA",
		"SecretAccessKey": "super-secret-key",
		"SessionToken": "super-secret-token",
		"Expiration": "2026-05-21T13:00:00Z",
	});
	let creds: CredentialProcessOutput = serde_json::from_value(wire.clone()).unwrap();

	// Secrets are readable through the inner value but never printed.
	assert_eq!(&*creds.secret_access_key, "super-secret-key");
	assert_eq!(&*creds.session_token, "super-secret-token");
	let debug = format!("{creds:?}");
	assert!(!debug.contains("super-secret-key"), "{debug}");
	assert!(!debug.contains("super-secret-token"), "{debug}");
	assert!(debug.contains("<redacted>"), "{debug}");

	// The expiry is a jiff timestamp, not a bare string.
	let _: jiff::Timestamp = creds.expiration;

	// Redaction and the timestamp rewrite are serialisation-transparent.
	assert_eq!(serde_json::to_value(&creds).unwrap(), wire);
}

#[test]
fn backup_target_repo_password_is_redacted() {
	let target: BackupTarget = serde_json::from_value(serde_json::json!({
		"storage": "s3",
		"bucket": "backups",
		"prefix": "",
		"region": "ap-southeast-2",
		"repo_password": "hunter2",
	}))
	.unwrap();
	assert_eq!(&*target.repo_password, "hunter2");
	let debug = format!("{target:?}");
	assert!(!debug.contains("hunter2"), "{debug}");
	assert!(debug.contains("<redacted>"), "{debug}");
}
