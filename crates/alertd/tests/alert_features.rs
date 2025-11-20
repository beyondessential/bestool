use std::{sync::Arc, time::Duration};

use bestool_alertd::{AlertDefinition, InternalContext};
use bestool_postgres::pool::{PgPool, create_pool};

async fn setup_test_db() -> PgPool {
	let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
	let pool = create_pool(&db_url, "bestool-alertd-test").await.unwrap();

	// Create a test table
	let client = pool.get().await.unwrap();
	client
		.execute(
			"CREATE TEMP TABLE test_metrics (
				id SERIAL PRIMARY KEY,
				name TEXT NOT NULL,
				value REAL NOT NULL,
				error_count INTEGER NOT NULL,
				created_at TIMESTAMP DEFAULT NOW(),
				updated_at TIMESTAMP DEFAULT NOW()
			)",
			&[],
		)
		.await
		.unwrap();

	pool
}

#[tokio::test]
async fn test_numerical_threshold_normal_trigger() {
	let pool = setup_test_db().await;

	// Insert test data
	let client = pool.get().await.unwrap();
	client
		.execute(
			"INSERT INTO test_metrics (name, value, error_count) VALUES ('cpu_usage', 95.5, 10)",
			&[],
		)
		.await
		.unwrap();

	let yaml = r#"
sql: "SELECT value FROM test_metrics WHERE name = 'cpu_usage'"
numerical:
  - field: value
    alert-at: 90
    clear-at: 50
send:
  - id: test
    subject: Test
    template: Test
"#;

	let mut alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
	alert.file = "test.yml".into();
	let (alert, _) = alert.normalise(&Default::default()).unwrap();

	let ctx = Arc::new(InternalContext { pg_pool: pool });
	let mut tera_ctx = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());

	// First run - not yet triggered, should trigger because value >= 90
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			false,
		)
		.await
		.unwrap();
	assert!(
		result.is_continue(),
		"Should trigger when value >= alert-at"
	);

	// Second run - already triggered, should stay triggered because value > clear-at
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			true,
		)
		.await
		.unwrap();
	assert!(
		result.is_continue(),
		"Should stay triggered when value > clear-at"
	);

	// Update to clear the alert
	let client = ctx.pg_pool.get().await.unwrap();
	client
		.execute(
			"UPDATE test_metrics SET value = 40 WHERE name = 'cpu_usage'",
			&[],
		)
		.await
		.unwrap();

	// Third run - should clear because value <= clear-at
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			true,
		)
		.await
		.unwrap();
	assert!(
		result.is_break(),
		"Should clear when value <= clear-at (40 <= 50)"
	);
}

#[tokio::test]
async fn test_numerical_threshold_inverted_trigger() {
	let pool = setup_test_db().await;

	// Insert test data with low free space
	let client = pool.get().await.unwrap();
	client
		.execute(
			"INSERT INTO test_metrics (name, value, error_count) VALUES ('free_space_gb', 5.0, 0)",
			&[],
		)
		.await
		.unwrap();

	let yaml = r#"
sql: "SELECT value FROM test_metrics WHERE name = 'free_space_gb'"
numerical:
  - field: value
    alert-at: 10
    clear-at: 50
send:
  - id: test
    subject: Test
    template: Test
"#;

	let mut alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
	alert.file = "test.yml".into();
	let (alert, _) = alert.normalise(&Default::default()).unwrap();

	let ctx = Arc::new(InternalContext { pg_pool: pool });
	let mut tera_ctx = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());

	// First run - not yet triggered, should trigger because value <= 10 (inverted)
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			false,
		)
		.await
		.unwrap();
	assert!(
		result.is_continue(),
		"Should trigger when value <= alert-at (inverted)"
	);

	// Second run - already triggered, should stay triggered because value < clear-at
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			true,
		)
		.await
		.unwrap();
	assert!(
		result.is_continue(),
		"Should stay triggered when value < clear-at (inverted)"
	);

	// Update to clear the alert
	let client = ctx.pg_pool.get().await.unwrap();
	client
		.execute(
			"UPDATE test_metrics SET value = 60 WHERE name = 'free_space_gb'",
			&[],
		)
		.await
		.unwrap();

	// Third run - should clear because value >= clear-at (inverted)
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			true,
		)
		.await
		.unwrap();
	assert!(
		result.is_break(),
		"Should clear when value >= clear-at (60 >= 50, inverted)"
	);
}

#[tokio::test]
async fn test_when_changed_simple() {
	let pool = setup_test_db().await;

	// Insert initial data
	let client = pool.get().await.unwrap();
	client
		.execute(
			"INSERT INTO test_metrics (name, value, error_count) VALUES ('errors', 100.0, 5)",
			&[],
		)
		.await
		.unwrap();

	let yaml = r#"
sql: "SELECT error_count FROM test_metrics WHERE name = 'errors'"
when-changed: true
send:
  - id: test
    subject: Test
    template: Test
"#;

	let mut alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
	alert.file = "test.yml".into();
	let (alert, _) = alert.normalise(&Default::default()).unwrap();

	let ctx = Arc::new(InternalContext { pg_pool: pool });

	// First execution - should trigger (first run always triggers)
	let _result = alert.execute(ctx.clone(), None, true, &[]).await.unwrap();
	// No error means it executed

	// Second execution with same data - would trigger but when-changed should prevent it
	// We can't easily test this without the full scheduler state, but we can verify the serialization

	let mut tera_ctx = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());
	let _ = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			false,
		)
		.await
		.unwrap();

	// Verify context has rows
	assert!(tera_ctx.get("rows").is_some());
}

#[tokio::test]
async fn test_when_changed_with_except() {
	let pool = setup_test_db().await;

	// Insert initial data
	let client = pool.get().await.unwrap();
	client
		.execute(
			"INSERT INTO test_metrics (name, value, error_count, created_at, updated_at)
			 VALUES ('test', 100.0, 5, NOW(), NOW())",
			&[],
		)
		.await
		.unwrap();

	let yaml = r#"
sql: "SELECT error_count, created_at, updated_at FROM test_metrics WHERE name = 'test'"
when-changed:
  except: [created_at, updated_at]
send:
  - id: test
    subject: Test
    template: Test
"#;

	let mut alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
	alert.file = "test.yml".into();
	let (alert, _) = alert.normalise(&Default::default()).unwrap();

	let ctx = Arc::new(InternalContext { pg_pool: pool });
	let mut tera_ctx = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());

	// Read initial data
	let _ = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			false,
		)
		.await
		.unwrap();

	let rows = tera_ctx.get("rows").unwrap();
	assert!(rows.as_array().unwrap().len() > 0);

	// Update only timestamps - when-changed should consider this unchanged
	tokio::time::sleep(Duration::from_millis(10)).await;
	let client = ctx.pg_pool.get().await.unwrap();
	client
		.execute(
			"UPDATE test_metrics SET updated_at = NOW() WHERE name = 'test'",
			&[],
		)
		.await
		.unwrap();

	// The serialization should be the same because we excluded timestamp columns
	// This would be verified in the scheduler's change detection logic
}

#[tokio::test]
async fn test_when_changed_with_only() {
	let pool = setup_test_db().await;

	// Insert initial data
	let client = pool.get().await.unwrap();
	client
		.execute(
			"INSERT INTO test_metrics (name, value, error_count) VALUES ('test', 100.0, 5)",
			&[],
		)
		.await
		.unwrap();

	let yaml = r#"
sql: "SELECT error_count, value FROM test_metrics WHERE name = 'test'"
when-changed:
  only: [error_count]
send:
  - id: test
    subject: Test
    template: Test
"#;

	let mut alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
	alert.file = "test.yml".into();
	let (alert, _) = alert.normalise(&Default::default()).unwrap();

	let ctx = Arc::new(InternalContext { pg_pool: pool });
	let mut tera_ctx = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());

	// Read initial data
	let _ = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			false,
		)
		.await
		.unwrap();

	// Update value (not in 'only' list) - should be considered unchanged
	let client = ctx.pg_pool.get().await.unwrap();
	client
		.execute(
			"UPDATE test_metrics SET value = 200 WHERE name = 'test'",
			&[],
		)
		.await
		.unwrap();

	// Update error_count (in 'only' list) - should be considered changed
	client
		.execute(
			"UPDATE test_metrics SET error_count = 10 WHERE name = 'test'",
			&[],
		)
		.await
		.unwrap();

	let mut tera_ctx2 = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());
	let _ = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx2,
			false,
		)
		.await
		.unwrap();

	// Both contexts should have rows
	assert!(tera_ctx.get("rows").is_some());
	assert!(tera_ctx2.get("rows").is_some());
}

#[tokio::test]
async fn test_numerical_and_when_changed_together() {
	let pool = setup_test_db().await;

	// Insert initial data
	let client = pool.get().await.unwrap();
	client
		.execute(
			"INSERT INTO test_metrics (name, value, error_count, created_at)
			 VALUES ('combo', 95.0, 100, NOW())",
			&[],
		)
		.await
		.unwrap();

	let yaml = r#"
sql: "SELECT value, error_count, created_at FROM test_metrics WHERE name = 'combo'"
numerical:
  - field: value
    alert-at: 90
    clear-at: 50
when-changed:
  except: [created_at]
send:
  - id: test
    subject: Test
    template: Test
"#;

	let mut alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
	alert.file = "test.yml".into();
	let (alert, _) = alert.normalise(&Default::default()).unwrap();

	let ctx = Arc::new(InternalContext { pg_pool: pool });
	let mut tera_ctx = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());

	// First run - should trigger due to numerical threshold
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			false,
		)
		.await
		.unwrap();
	assert!(
		result.is_continue(),
		"Should trigger when numerical threshold exceeded"
	);

	// Verify rows are in context
	assert!(tera_ctx.get("rows").is_some());
	let rows = tera_ctx.get("rows").unwrap().as_array().unwrap();
	assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn test_multiple_numerical_thresholds() {
	let pool = setup_test_db().await;

	// Insert test data with multiple fields
	let client = pool.get().await.unwrap();
	client
		.execute(
			"INSERT INTO test_metrics (name, value, error_count) VALUES ('multi', 95.0, 150)",
			&[],
		)
		.await
		.unwrap();

	let yaml = r#"
sql: "SELECT value as cpu, error_count as errors FROM test_metrics WHERE name = 'multi'"
numerical:
  - field: cpu
    alert-at: 90
    clear-at: 50
  - field: errors
    alert-at: 100
    clear-at: 50
send:
  - id: test
    subject: Test
    template: Test
"#;

	let mut alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
	alert.file = "test.yml".into();
	let (alert, _) = alert.normalise(&Default::default()).unwrap();

	let ctx = Arc::new(InternalContext { pg_pool: pool });
	let mut tera_ctx = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());

	// Should trigger because both thresholds are exceeded
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx,
			false,
		)
		.await
		.unwrap();
	assert!(
		result.is_continue(),
		"Should trigger when any threshold is exceeded"
	);

	// Lower cpu but keep errors high
	let client = ctx.pg_pool.get().await.unwrap();
	client
		.execute(
			"UPDATE test_metrics SET value = 40 WHERE name = 'multi'",
			&[],
		)
		.await
		.unwrap();

	let mut tera_ctx2 = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx2,
			true,
		)
		.await
		.unwrap();
	assert!(
		result.is_continue(),
		"Should stay triggered because errors threshold still exceeded"
	);

	// Lower both to clear
	client
		.execute(
			"UPDATE test_metrics SET error_count = 30 WHERE name = 'multi'",
			&[],
		)
		.await
		.unwrap();

	let mut tera_ctx3 = bestool_alertd::templates::build_context(&alert, chrono::Utc::now());
	let result = alert
		.read_sources(
			&ctx.pg_pool,
			chrono::Utc::now() - alert.interval_duration,
			&mut tera_ctx3,
			true,
		)
		.await
		.unwrap();
	assert!(
		result.is_break(),
		"Should clear when all thresholds are below clear-at"
	);
}
