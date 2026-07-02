//! Proves the build-time generated `schema` layer exists, has the expected
//! shape, and round-trips through serde. These run against the committed
//! snapshot: generation already happened at build time, so no network is
//! needed here.

use bestool_canopy::schema::{
	BackupPurpose, BackupTarget, BeginArgs, BeginResponse, CompleteArgs, CompleteResponse, Issue,
	ReportArgs, RunOutcome,
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
fn datetime_fields_deserialise_into_jiff_timestamp() {
	let issue: Issue = serde_json::from_value(serde_json::json!({
		"active": true,
		"created_at": "2026-01-02T03:04:05Z",
		"first_seen": "2026-01-02T03:04:05Z",
		"last_seen": "2026-01-02T03:04:05Z",
		"id": "6f8a2b1c-0000-4000-8000-000000000000",
		"message": "disk full",
		"ref": "host/alert:disk",
		"severity": "warning",
		"source": "alertd",
		"updated_at": "2026-01-02T03:04:05Z",
	}))
	.unwrap();

	let created: jiff::Timestamp = issue.created_at;
	assert_eq!(created, "2026-01-02T03:04:05Z".parse().unwrap());
}
