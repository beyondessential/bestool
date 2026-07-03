//! Contract tests against the live canopy OpenAPI spec.
//!
//! Fetches `https://meta.tamanu.app/api/openapi.json` and checks that every
//! canopy endpoint bestool calls exists, that the payloads bestool sends
//! validate against the spec's request schemas, and that spec-valid response
//! samples decode into bestool's types. These tests need network access, and
//! fail honestly when live canopy doesn't (yet) serve an endpoint bestool
//! depends on.
//!
//! All tests here are `#[ignore]`d so plain `cargo test` skips them; CI runs
//! them in a dedicated job (`cargo test -p bestool --lib canopy_contract --
//! --ignored`) so a contract failure is clearly drift against live canopy
//! rather than a fault in bestool's own test suite.

use std::collections::BTreeMap;

use bestool_canopy::schema::{
	BackupPurpose, BackupTarget, BeginArgs, BeginResponse, CapabilitiesArgs, CompleteArgs,
	CompleteResponse, CredentialProcessOutput, CredentialsArgs, NewEvent, ReportArgs, RunOutcome,
	Severity,
};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::actions::tamanu::{artifacts::Artifact, psql::Snippet};

const SPEC_URL: &str = "https://meta.tamanu.app/api/openapi.json";

async fn spec() -> &'static Value {
	static SPEC: OnceCell<Value> = OnceCell::const_new();
	SPEC.get_or_init(|| async {
		reqwest::Client::new()
			.get(SPEC_URL)
			.send()
			.await
			.expect("fetching live canopy spec")
			.error_for_status()
			.expect("fetching live canopy spec")
			.json()
			.await
			.expect("parsing live canopy spec")
	})
	.await
}

/// Escape a path segment for use in a JSON pointer (RFC 6901).
fn escape(segment: &str) -> String {
	segment.replace('~', "~0").replace('/', "~1")
}

/// Look up `pointer` in the spec, resolving a single level of `$ref`.
fn resolve<'a>(spec: &'a Value, pointer: &str) -> &'a Value {
	let value = spec
		.pointer(pointer)
		.unwrap_or_else(|| panic!("live canopy spec is missing {pointer}"));
	match value.get("$ref").and_then(Value::as_str) {
		Some(target) => spec
			.pointer(target.trim_start_matches('#'))
			.unwrap_or_else(|| panic!("dangling $ref {target} in live canopy spec")),
		None => value,
	}
}

/// Compile a validator for the schema at `pointer` within the spec.
///
/// The target schema is cloned out as the root, with the spec's `components`
/// grafted alongside it so internal `#/components/schemas/...` references
/// resolve. (A root `$ref` to the pointer would be neater, but path templates
/// like `{server_id}` aren't valid in `$ref` URIs.)
fn validator_at(spec: &Value, pointer: &str) -> jsonschema::Validator {
	let mut root = spec
		.pointer(pointer)
		.unwrap_or_else(|| panic!("live canopy spec is missing {pointer}"))
		.clone();
	root.as_object_mut()
		.unwrap_or_else(|| panic!("schema at {pointer} is not an object"))
		.insert("components".into(), spec["components"].clone());
	jsonschema::validator_for(&root)
		.unwrap_or_else(|err| panic!("compiling schema at {pointer}: {err}"))
}

fn assert_operation_exists(spec: &Value, path: &str, method: &str) {
	let pointer = format!("/paths/{}/{method}", escape(path));
	assert!(
		spec.pointer(&pointer).is_some(),
		"live canopy does not serve {} {path}",
		method.to_uppercase(),
	);
}

fn assert_valid(spec: &Value, pointer: &str, instance: &Value) {
	let validator = validator_at(spec, pointer);
	let errors: Vec<String> = validator
		.iter_errors(instance)
		.map(|err| format!("{err} (at instance path `{}`)", err.instance_path()))
		.collect();
	assert!(
		errors.is_empty(),
		"instance does not validate against {pointer}:\n{errors:#?}\ninstance: {instance:#}",
	);
}

fn request_schema(path: &str, method: &str) -> String {
	format!(
		"/paths/{}/{method}/requestBody/content/application~1json/schema",
		escape(path),
	)
}

fn response_schema(path: &str, method: &str) -> String {
	format!(
		"/paths/{}/{method}/responses/200/content/application~1json/schema",
		escape(path),
	)
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn events_request_matches_spec() {
	let spec = spec().await;
	assert_operation_exists(spec, "/events", "post");

	let event = NewEvent {
		source: "bestool-contract-test".to_owned(),
		ref_: "host/alert:target".to_owned(),
		message: "message".to_owned(),
		description: Some("description".to_owned()),
		severity: Some(Severity::Warning),
		occurred_at: Some("2026-01-01T00:00:00Z".parse().unwrap()),
		active: Some(true),
	};
	let instance = serde_json::to_value(&event).unwrap();
	assert_valid(spec, &request_schema("/events", "post"), &instance);

	// Negative case, proving the validation isn't vacuous: a retired syslog
	// severity must be rejected.
	let mut invalid = instance.clone();
	invalid["severity"] = json!("notice");
	let validator = validator_at(spec, &request_schema("/events", "post"));
	assert!(
		!validator.is_valid(&invalid),
		"spec validation accepted a retired severity; the validator is not checking refs",
	);
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn severity_vocabulary_matches_spec() {
	let spec = spec().await;
	let spec_levels: Vec<&str> = resolve(spec, "/components/schemas/Severity")
		.get("enum")
		.and_then(Value::as_array)
		.expect("Severity schema has an enum")
		.iter()
		.map(|v| v.as_str().expect("Severity enum values are strings"))
		.collect();

	// Exhaustive match: adding a Severity variant breaks this and forces the
	// list below to be updated.
	use Severity::*;
	const ALL: &[Severity] = &[Critical, Error, Warning, Info, Debug];
	for severity in ALL {
		match severity {
			Critical | Error | Warning | Info | Debug => {}
		}
	}

	let ours: Vec<String> = ALL
		.iter()
		.map(|s| {
			serde_json::to_value(s)
				.unwrap()
				.as_str()
				.unwrap()
				.to_owned()
		})
		.collect();

	for level in &spec_levels {
		assert!(
			serde_json::from_value::<Severity>(json!(level)).is_ok(),
			"canopy severity {level:?} does not deserialise into bestool's Severity",
		);
	}
	for level in &ours {
		assert!(
			spec_levels.contains(&level.as_str()),
			"bestool severity {level:?} is not accepted by canopy (spec has {spec_levels:?})",
		);
	}
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn status_request_matches_spec() {
	let spec = spec().await;
	assert_operation_exists(spec, "/status/{server_id}", "post");

	// Representative sweep payload: the reserved `health` key plus free-form
	// extras, as posted by alertd's doctor task.
	let instance = json!({
		"health": [
			{"check": "uptime", "result": "passed", "uptime_secs": 12345},
			{"check": "disk", "result": "failed", "free_percent": 3},
			{"check": "sync_lookup", "result": "broken"},
			{"check": "fhir_jobs", "result": "skipped"},
			{"check": "load", "result": "warning"},
		],
		"hostname": "test-host",
		"pg_version": "16.4",
	});
	assert_valid(
		spec,
		&request_schema("/status/{server_id}", "post"),
		&instance,
	);
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn servers_probe_path_exists() {
	// `GET /servers` is the no-auth probe `CanopyClient` uses to detect the
	// tailscale path.
	assert_operation_exists(spec().await, "/servers", "get");
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn register_begin_matches_spec() {
	let spec = spec().await;
	assert_operation_exists(spec, "/servers/register/begin", "post");

	let request = BeginArgs {
		server_id: "00000000-0000-0000-0000-000000000000".parse().unwrap(),
		token: "enrollment-token".to_owned(),
		spki: Some("c3BraQ==".to_owned()),
	};
	let instance = serde_json::to_value(&request).unwrap();
	assert_valid(
		spec,
		&request_schema("/servers/register/begin", "post"),
		&instance,
	);

	let sample = json!({"nonce": "bm9uY2U=", "channel_binding_required": true});
	assert_valid(
		spec,
		&response_schema("/servers/register/begin", "post"),
		&sample,
	);
	let decoded: BeginResponse = serde_json::from_value(sample).unwrap();
	assert_eq!(decoded.nonce, "bm9uY2U=");
	assert!(decoded.channel_binding_required);
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn register_complete_matches_spec() {
	let spec = spec().await;
	assert_operation_exists(spec, "/servers/register/complete", "post");

	let request = CompleteArgs {
		server_id: "00000000-0000-0000-0000-000000000000".parse().unwrap(),
		nonce: "bm9uY2U=".to_owned(),
		signature: "c2lnbmF0dXJl".to_owned(),
		spki: None,
	};
	let instance = serde_json::to_value(&request).unwrap();
	assert_valid(
		spec,
		&request_schema("/servers/register/complete", "post"),
		&instance,
	);

	let sample = json!({
		"server_id": "00000000-0000-0000-0000-000000000000",
		"device_id": "11111111-1111-1111-1111-111111111111",
	});
	assert_valid(
		spec,
		&response_schema("/servers/register/complete", "post"),
		&sample,
	);
	let decoded: CompleteResponse = serde_json::from_value(sample).unwrap();
	assert_eq!(
		decoded.server_id.to_string(),
		"00000000-0000-0000-0000-000000000000"
	);
	assert_eq!(
		decoded.device_id.to_string(),
		"11111111-1111-1111-1111-111111111111"
	);
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn tags_response_matches_decode() {
	let spec = spec().await;
	assert_operation_exists(spec, "/tags", "get");

	let sample = json!({"role": "central", "fleet": "test"});
	assert_valid(spec, &response_schema("/tags", "get"), &sample);
	let decoded: BTreeMap<String, String> = serde_json::from_value(sample).unwrap();
	assert_eq!(decoded["role"], "central");

	// The schema must be a string→string map, matching what `tamanu tags`
	// decodes; anything else (like the bare list it used to be) is drift.
	let schema = resolve(spec, &response_schema("/tags", "get"));
	assert_eq!(
		schema.pointer("/additionalProperties/type"),
		Some(&json!("string")),
		"canopy /tags response is no longer a string→string map: {schema:#}",
	);
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn snippets_response_matches_decode() {
	let spec = spec().await;
	assert_operation_exists(spec, "/bestool/snippets", "get");

	let sample = json!({
		"slow-queries": {"sql": "select 1", "description": "example"},
		"bare": {"sql": "select 2", "description": null},
	});
	assert_valid(spec, &response_schema("/bestool/snippets", "get"), &sample);
	let decoded: BTreeMap<String, Snippet> = serde_json::from_value(sample).unwrap();
	assert_eq!(decoded["slow-queries"].sql, "select 1");
	assert_eq!(decoded["bare"].description, None);
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn artifacts_response_matches_decode() {
	let spec = spec().await;
	assert_operation_exists(spec, "/versions/{version}/artifacts", "get");

	let sample = json!([{
		"id": "00000000-0000-0000-0000-000000000000",
		"artifact_type": "central",
		"platform": "linux-x86_64",
		"download_url": "https://example.com/artifact.tar.gz",
	}]);
	assert_valid(
		spec,
		&response_schema("/versions/{version}/artifacts", "get"),
		&sample,
	);
	let decoded: Vec<Artifact> = serde_json::from_value(sample).unwrap();
	assert_eq!(decoded[0].artifact_type, "central");
	assert_eq!(
		decoded[0].download_url.as_str(),
		"https://example.com/artifact.tar.gz",
	);
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn backup_capabilities_request_matches_spec() {
	let spec = spec().await;
	assert_operation_exists(spec, "/backup-capabilities", "post");

	let types = vec!["tamanu-postgres".to_owned()];
	let instance = serde_json::to_value(CapabilitiesArgs { types }).unwrap();
	assert_valid(
		spec,
		&request_schema("/backup-capabilities", "post"),
		&instance,
	);
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn backup_credentials_request_and_response_match_spec() {
	let spec = spec().await;
	assert_operation_exists(spec, "/backup-credentials", "post");

	let instance = serde_json::to_value(CredentialsArgs {
		type_: "tamanu-postgres".to_owned(),
		purpose: Some(BackupPurpose::Backup),
	})
	.unwrap();
	assert_valid(
		spec,
		&request_schema("/backup-credentials", "post"),
		&instance,
	);

	// Negative case, proving the validation isn't vacuous: an invalid purpose
	// must be rejected.
	let mut invalid = instance.clone();
	invalid["purpose"] = json!("sideways");
	let validator = validator_at(spec, &request_schema("/backup-credentials", "post"));
	assert!(
		!validator.is_valid(&invalid),
		"spec accepted an invalid purpose; the validator is not checking refs",
	);

	// A spec-valid credential_process response decodes into BackupCredentials.
	let sample = json!({
		"Version": 1,
		"AccessKeyId": "AKIA",
		"SecretAccessKey": "secret",
		"SessionToken": "token",
		"Expiration": "2026-05-21T13:00:00Z",
	});
	assert_valid(
		spec,
		&response_schema("/backup-credentials", "post"),
		&sample,
	);
	let decoded: CredentialProcessOutput = serde_json::from_value(sample).unwrap();
	assert_eq!(decoded.version, 1);
	assert_eq!(&*decoded.session_token, "token");
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn backup_target_response_matches_decode() {
	let spec = spec().await;
	assert_operation_exists(spec, "/backup-target", "get");

	let sample = json!({
		"storage": "s3",
		"bucket": "my-bucket",
		"prefix": "",
		"region": "ap-southeast-2",
		"repo_password": "hunter2",
	});
	assert_valid(spec, &response_schema("/backup-target", "get"), &sample);
	let decoded: BackupTarget = serde_json::from_value(sample).unwrap();
	assert_eq!(decoded.bucket, "my-bucket");
	assert_eq!(&*decoded.repo_password, "hunter2");
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn backup_report_request_matches_spec() {
	let spec = spec().await;
	assert_operation_exists(spec, "/backup-report", "post");

	let instance = serde_json::to_value(ReportArgs {
		run_id: "00000000-0000-0000-0000-000000000000".parse().unwrap(),
		type_: "tamanu-postgres".to_owned(),
		purpose: BackupPurpose::Backup,
		outcome: RunOutcome::Success,
		error: None,
		bytes_uploaded: Some(12345),
		snapshot_id: Some("abc123".to_owned()),
		// Intentionally populated: this gates the PR. bestool already sends these
		// (canopy ignores unknown fields), but the contract check stays red until
		// canopy's OpenAPI spec defines the s3 traffic fields — then it goes green.
		s3_sent_raw_bytes: Some(2_000_000),
		s3_sent_payload_bytes: Some(1_950_000),
		s3_received_raw_bytes: Some(4096),
		s3_received_payload_bytes: Some(0),
	})
	.unwrap();
	assert_valid(spec, &request_schema("/backup-report", "post"), &instance);

	// Negative case: an invalid outcome must be rejected.
	let mut invalid = instance.clone();
	invalid["outcome"] = json!("maybe");
	let validator = validator_at(spec, &request_schema("/backup-report", "post"));
	assert!(
		!validator.is_valid(&invalid),
		"spec accepted an invalid outcome; the validator is not checking refs",
	);
}

#[tokio::test]
#[ignore = "live canopy contract test; run by the dedicated CI job"]
async fn status_response_carries_backup_now() {
	// The "back up now" trigger reads `backup_now` off the status response;
	// confirm canopy still serves it as a string array.
	let spec = spec().await;
	let schema = resolve(spec, &response_schema("/status/{server_id}", "post"));
	// The response flattens `Status` + `backup_now`, so it's serialised as an
	// `allOf` — look for `backup_now` directly or in any `allOf` member.
	let backup_now = schema
		.pointer("/properties/backup_now")
		.or_else(|| {
			schema
				.get("allOf")?
				.as_array()?
				.iter()
				.find_map(|member| member.pointer("/properties/backup_now"))
		})
		.unwrap_or_else(|| panic!("status response has no backup_now: {schema:#}"));
	assert_eq!(
		backup_now.pointer("/type"),
		Some(&json!("array")),
		"backup_now is no longer an array: {backup_now:#}",
	);
	assert_eq!(
		backup_now.pointer("/items/type"),
		Some(&json!("string")),
		"backup_now items are no longer strings: {backup_now:#}",
	);
}
