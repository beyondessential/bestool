use std::collections::HashMap;

use bestool_alertd::{
	AlertDefinition, AlwaysSend, EventType, ExternalTarget, TargetConnection, TargetEmail,
	TicketSource, WhenChanged,
};

#[test]
fn test_database_down_event_type_parsing() {
	let yaml = "database-down";
	let event: EventType = serde_yaml::from_str(yaml).unwrap();
	assert_eq!(event, EventType::DatabaseDown);
}

#[test]
fn test_database_down_event_alert_definition() {
	let yaml = r#"
event: database-down
send:
  - id: test-target
    subject: "DB Down: {{ hostname }}"
    template: "Database {{ database_url }} is unreachable: {{ error_message }}"
"#;
	let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
	assert!(matches!(
		alert.source,
		TicketSource::Event {
			event: EventType::DatabaseDown
		}
	));
}

#[test]
fn test_database_down_default_template_renders() {
	let subject_template = "[bestool-alertd] {{ hostname }}: Database unreachable";
	let body_template = "The PostgreSQL database that alertd depends on is unreachable.\n\n\
		Database URL: {{ database_url }}\n\
		Error: <pre>{{ error_message }}</pre>\n\n\
		All SQL-based alerts are non-functional until the database is restored.";

	let tera = bestool_alertd::templates::load_templates(
		&Some(subject_template.to_string()),
		body_template,
	)
	.unwrap();

	let synthetic_alert = AlertDefinition {
		file: "[internal:database-down]".into(),
		enabled: true,
		interval: "0 seconds".to_string(),
		interval_duration: std::time::Duration::from_secs(0),
		always_send: AlwaysSend::Boolean(false),
		when_changed: WhenChanged::default(),
		send: Vec::new(),
		source: TicketSource::Event {
			event: EventType::DatabaseDown,
		},
	};

	let mut ctx = bestool_alertd::templates::build_context(&synthetic_alert, chrono::Utc::now());
	ctx.insert("database_url", "postgresql://user:***@localhost/mydb");
	ctx.insert("error_message", "connection refused");

	let (subject, body) = bestool_alertd::templates::render_alert(&tera, &mut ctx).unwrap();

	assert!(
		subject.contains("Database unreachable"),
		"Subject should mention database unreachable, got: {subject}"
	);
	assert!(
		body.contains("postgresql://user:***@localhost/mydb"),
		"Body should contain the (redacted) database URL, got: {body}"
	);
	assert!(
		body.contains("connection refused"),
		"Body should contain the error message, got: {body}"
	);
	assert!(
		body.contains("All SQL-based alerts are non-functional"),
		"Body should mention alerts are non-functional, got: {body}"
	);
}

#[test]
fn test_database_down_event_alert_normalises_with_targets() {
	let yaml = r#"
event: database-down
send:
  - id: ops
    subject: "DB DOWN"
    template: "The database is down: {{ error_message }}"
"#;
	let mut alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
	alert.file = "db-down-alert.yml".into();

	let mut external_targets = HashMap::new();
	external_targets.insert(
		"ops".to_string(),
		vec![ExternalTarget {
			id: "ops".to_string(),
			conn: TargetConnection::Email(TargetEmail {
				addresses: vec!["ops@example.com".to_string()],
			}),
		}],
	);

	let (_alert, resolved) = alert.normalise(&external_targets).unwrap();
	assert!(
		!resolved.is_empty(),
		"Should resolve at least one target for the database-down event alert"
	);
}

#[tokio::test]
async fn test_health_check_detects_unreachable_database() {
	let bad_url = "postgresql://localhost:59999/nonexistent?connect_timeout=1";
	let pool_result =
		bestool_postgres::pool::create_pool(bad_url, "bestool-alertd-health-test").await;

	assert!(
		pool_result.is_err(),
		"Connecting to a non-existent database should fail"
	);
}

#[tokio::test]
async fn test_health_check_succeeds_on_valid_database() {
	let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
	let pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-health-test")
		.await
		.unwrap();

	let conn = pool
		.get_timeout(std::time::Duration::from_secs(5))
		.await
		.expect("should get a connection from pool");
	let result = conn.simple_query("SELECT 1").await;
	assert!(result.is_ok(), "SELECT 1 health check should succeed");
}

#[test]
fn test_database_url_password_redaction() {
	let url_with_password = "postgresql://user:secretpass@localhost:5432/mydb";
	let mut parsed = url::Url::parse(url_with_password).unwrap();
	let _ = parsed.set_password(Some("***"));
	let redacted = parsed.to_string();

	assert!(
		!redacted.contains("secretpass"),
		"Password should be redacted, got: {redacted}"
	);
	assert!(
		redacted.contains("***"),
		"Redacted password should show ***, got: {redacted}"
	);
	assert!(
		redacted.contains("user"),
		"Username should be preserved, got: {redacted}"
	);
	assert!(
		redacted.contains("localhost"),
		"Host should be preserved, got: {redacted}"
	);
}

#[test]
fn test_database_url_redaction_without_password() {
	let url_without_password = "postgresql://localhost/mydb";
	let parsed = url::Url::parse(url_without_password).unwrap();
	assert!(parsed.password().is_none());
	let result = parsed.to_string();
	assert!(
		!result.contains("***"),
		"Should not add *** when no password present, got: {result}"
	);
}

#[test]
fn test_database_url_redaction_unparseable() {
	let bad_url = "not a url at all";
	let result = match url::Url::parse(bad_url) {
		Ok(mut parsed) => {
			if parsed.password().is_some() {
				let _ = parsed.set_password(Some("***"));
			}
			parsed.to_string()
		}
		Err(_) => "(unparseable)".to_string(),
	};
	assert_eq!(result, "(unparseable)");
}
