use std::path::PathBuf;

use chrono::{Duration, Utc};

use super::{
	definition::{AlertDefinition, TicketSource},
	targets::SendTarget,
	templates::build_context,
};

fn interval_context(dur: Duration) -> Option<String> {
	let alert = AlertDefinition {
		file: PathBuf::from("test.yaml"),
		enabled: true,
		interval: dur.to_std().unwrap(),
		source: TicketSource::Sql { sql: "".into() },
		send: vec![],
	};
	build_context(&alert, Utc::now())
		.get("interval")
		.and_then(|v| v.as_str())
		.map(|s| s.to_owned())
}

#[test]
fn test_interval_format_minutes() {
	assert_eq!(
		interval_context(Duration::minutes(15)).as_deref(),
		Some("15m"),
	);
}

#[test]
fn test_interval_format_hour() {
	assert_eq!(interval_context(Duration::hours(1)).as_deref(), Some("1h"),);
}

#[test]
fn test_interval_format_day() {
	assert_eq!(interval_context(Duration::days(1)).as_deref(), Some("1d"),);
}

#[test]
fn test_alert_parse_email() {
	let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: email
  addresses: [test@example.com]
  subject: "[Tamanu Alert] Example ({{ hostname }})"
  template: |
    <p>Server: {{ hostname }}</p>
    <p>There are {{ rows | length }} rows.</p>
"#;
	let alert: AlertDefinition = serde_yaml::from_str(&alert).unwrap();
	let alert = alert.normalise(&Default::default());
	assert_eq!(alert.interval, std::time::Duration::default());
	assert!(matches!(alert.source, TicketSource::Sql { sql } if sql == "SELECT $1::timestamptz;"));
	assert!(matches!(alert.send[0], SendTarget::Email { .. }));
}

#[test]
fn test_alert_parse_shell() {
	let alert = r#"
shell: bash
run: echo foobar
"#;
	let alert: AlertDefinition = serde_yaml::from_str(&alert).unwrap();
	let alert = alert.normalise(&Default::default());
	assert_eq!(alert.interval, std::time::Duration::default());
	assert!(
		matches!(alert.source, TicketSource::Shell { shell, run } if shell == "bash" && run == "echo foobar")
	);
}

#[test]
fn test_alert_parse_invalid_source() {
	let alert = r#"
shell: bash
"#;
	assert!(matches!(
		serde_yaml::from_str::<AlertDefinition>(&alert),
		Err(_)
	));
	let alert = r#"
run: echo foo
"#;
	assert!(matches!(
		serde_yaml::from_str::<AlertDefinition>(&alert),
		Err(_)
	));
	let alert = r#"
sql: SELECT $1::timestamptz;
run: echo foo
"#;
	assert!(matches!(
		serde_yaml::from_str::<AlertDefinition>(&alert),
		Err(_)
	));
	let alert = r#"
sql: SELECT $1::timestamptz;
shell: bash
"#;
	assert!(matches!(
		serde_yaml::from_str::<AlertDefinition>(&alert),
		Err(_)
	));
	let alert = r#"
sql: SELECT $1::timestamptz;
shell: bash
run: echo foo
"#;
	assert!(matches!(
		serde_yaml::from_str::<AlertDefinition>(&alert),
		Err(_)
	));
}

#[test]
fn test_alert_parse_zendesk_authorized() {
	let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: zendesk
  endpoint: https://example.zendesk.com/api/v2/requests
  credentials:
    email: foo@example.com
    password: pass
  subject: "[Tamanu Alert] Example ({{ hostname }})"
  template: "Output: {{ output }}""#;
	let alert: AlertDefinition = serde_yaml::from_str(&alert).unwrap();
	assert!(matches!(alert.send[0], SendTarget::Zendesk { .. }));
}

#[test]
fn test_alert_parse_zendesk_anon() {
	let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: zendesk
  endpoint: https://example.zendesk.com/api/v2/requests
  requester: "{{ hostname }}"
  subject: "[Tamanu Alert] Example ({{ hostname }})"
  template: "Output: {{ output }}""#;
	let alert: AlertDefinition = serde_yaml::from_str(&alert).unwrap();
	assert!(matches!(alert.send[0], SendTarget::Zendesk { .. }));
}

#[test]
fn test_alert_parse_zendesk_form_fields() {
	let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: zendesk
  endpoint: https://example.zendesk.com/api/v2/requests
  requester: "{{ hostname }}"
  subject: "[Tamanu Alert] Example ({{ hostname }})"
  template: "Output: {{ output }}"
  ticket_form_id: 500
  custom_fields:
  - id: 100
    value: tamanu_
  - id: 200
    value: Test
"#;
	let alert: AlertDefinition = serde_yaml::from_str(&alert).unwrap();
	assert!(matches!(alert.send[0], SendTarget::Zendesk { .. }));
}

#[test]
fn test_alert_parse_slack() {
	let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: slack
  webhook: https://hooks.slack.com/triggers/
  template: Something happened
"#;
	let alert: AlertDefinition = serde_yaml::from_str(&alert).unwrap();
	assert!(matches!(alert.send[0], SendTarget::Slack { .. }));
}

#[test]
fn test_alert_parse_slack_fields() {
	let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: slack
  webhook: https://hooks.slack.com/triggers/
  template: Something happened
  fields:
  - name: alertname
    field: filename
  - name: deployment
    value: production
"#;
	let alert: AlertDefinition = serde_yaml::from_str(&alert).unwrap();
	assert!(matches!(alert.send[0], SendTarget::Slack { .. }));
}
