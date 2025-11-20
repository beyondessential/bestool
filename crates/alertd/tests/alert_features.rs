use std::{sync::Arc, time::Duration};

use bestool_alertd::{AlertDefinition, InternalContext};
use bestool_postgres::pool::{PgPool, create_pool};

async fn setup_test_db(table_name: &str) -> (PgPool, String) {
	let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
	let pool = create_pool(&db_url, "bestool-alertd-test").await.unwrap();

	let client = pool.get().await.unwrap();

	// Create a unique test table for this test
	let create_sql = format!(
		"CREATE TABLE IF NOT EXISTS {} (
			id SERIAL PRIMARY KEY,
			name TEXT NOT NULL,
			value REAL NOT NULL,
			error_count INTEGER NOT NULL,
			created_at TIMESTAMP DEFAULT NOW(),
			updated_at TIMESTAMP DEFAULT NOW()
		)",
		table_name
	);
	client.execute(&create_sql, &[]).await.unwrap();

	// Clean up any existing test data in this table
	let delete_sql = format!("DELETE FROM {}", table_name);
	client.execute(&delete_sql, &[]).await.unwrap();

	(pool, table_name.to_string())
}

#[tokio::test]
async fn test_numerical_threshold_normal_trigger() {
	let (pool, table_name) = setup_test_db("test_metrics_normal").await;

	// Insert test data
	let client = pool.get().await.unwrap();
	let insert_sql = format!(
		"INSERT INTO {} (name, value, error_count) VALUES ('cpu_usage', 95.5, 10)",
		table_name
	);
	client.execute(&insert_sql, &[]).await.unwrap();

	let yaml = format!(
		r#"
sql: "SELECT value FROM {} WHERE name = 'cpu_usage'"
numerical:
  - field: value
    alert-at: 90
    clear-at: 50
send:
  - id: test
    subject: Test
    template: Test
"#,
		table_name
	);

	let mut alert: AlertDefinition = serde_yaml::from_str(&yaml).unwrap();
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
	let update_sql = format!(
		"UPDATE {} SET value = 40 WHERE name = 'cpu_usage'",
		table_name
	);
	client.execute(&update_sql, &[]).await.unwrap();

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
	let (pool, table_name) = setup_test_db("test_metrics_inverted").await;

	// Insert test data with low free space
	let client = pool.get().await.unwrap();
	let insert_sql = format!(
		"INSERT INTO {} (name, value, error_count) VALUES ('free_space_gb', 5.0, 0)",
		table_name
	);
	client.execute(&insert_sql, &[]).await.unwrap();

	let yaml = format!(
		r#"
sql: "SELECT value FROM {} WHERE name = 'free_space_gb'"
numerical:
  - field: value
    alert-at: 10
    clear-at: 50
send:
  - id: test
    subject: Test
    template: Test
"#,
		table_name
	);

	let mut alert: AlertDefinition = serde_yaml::from_str(&yaml).unwrap();
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
	let update_sql = format!(
		"UPDATE {} SET value = 60 WHERE name = 'free_space_gb'",
		table_name
	);
	client.execute(&update_sql, &[]).await.unwrap();

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
	let (pool, table_name) = setup_test_db("test_metrics_changed_simple").await;

	// Insert initial data
	let client = pool.get().await.unwrap();
	let insert_sql = format!(
		"INSERT INTO {} (name, value, error_count) VALUES ('errors', 100.0, 5)",
		table_name
	);
	client.execute(&insert_sql, &[]).await.unwrap();

	let yaml = format!(
		r#"
sql: "SELECT error_count FROM {} WHERE name = 'errors'"
when-changed: true
send:
  - id: test
    subject: Test
    template: Test
"#,
		table_name
	);

	let mut alert: AlertDefinition = serde_yaml::from_str(&yaml).unwrap();
	alert.file = "test.yml".into();
	let (alert, _) = alert.normalise(&Default::default()).unwrap();

	let ctx = Arc::new(InternalContext { pg_pool: pool });

	// First execution - should trigger (first run always triggers)
	alert.execute(ctx.clone(), None, true, &[]).await.unwrap();
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
	let (pool, table_name) = setup_test_db("test_metrics_changed_except").await;

	// Insert initial data
	let client = pool.get().await.unwrap();
	let insert_sql = format!(
		"INSERT INTO {} (name, value, error_count, created_at, updated_at)
		 VALUES ('test', 100.0, 5, NOW(), NOW())",
		table_name
	);
	client.execute(&insert_sql, &[]).await.unwrap();

	let yaml = format!(
		r#"
sql: "SELECT error_count, created_at, updated_at FROM {} WHERE name = 'test'"
when-changed:
  except: [created_at, updated_at]
send:
  - id: test
    subject: Test
    template: Test
"#,
		table_name
	);

	let mut alert: AlertDefinition = serde_yaml::from_str(&yaml).unwrap();
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
	assert!(!rows.as_array().unwrap().is_empty());

	// Update only timestamps - when-changed should consider this unchanged
	tokio::time::sleep(Duration::from_millis(10)).await;
	let client = ctx.pg_pool.get().await.unwrap();
	let update_sql = format!(
		"UPDATE {} SET updated_at = NOW() WHERE name = 'test'",
		table_name
	);
	client.execute(&update_sql, &[]).await.unwrap();

	// The serialization should be the same because we excluded timestamp columns
	// This would be verified in the scheduler's change detection logic
}

#[tokio::test]
async fn test_when_changed_with_only() {
	let (pool, table_name) = setup_test_db("test_metrics_changed_only").await;

	// Insert initial data
	let client = pool.get().await.unwrap();
	let insert_sql = format!(
		"INSERT INTO {} (name, value, error_count) VALUES ('test', 100.0, 5)",
		table_name
	);
	client.execute(&insert_sql, &[]).await.unwrap();

	let yaml = format!(
		r#"
sql: "SELECT error_count, value FROM {} WHERE name = 'test'"
when-changed:
  only: [error_count]
send:
  - id: test
    subject: Test
    template: Test
"#,
		table_name
	);

	let mut alert: AlertDefinition = serde_yaml::from_str(&yaml).unwrap();
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
	let update_sql1 = format!("UPDATE {} SET value = 200 WHERE name = 'test'", table_name);
	client.execute(&update_sql1, &[]).await.unwrap();

	// Update error_count (in 'only' list) - should be considered changed
	let update_sql2 = format!(
		"UPDATE {} SET error_count = 10 WHERE name = 'test'",
		table_name
	);
	client.execute(&update_sql2, &[]).await.unwrap();

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
	let (pool, table_name) = setup_test_db("test_metrics_combo").await;

	// Insert initial data
	let client = pool.get().await.unwrap();
	let insert_sql = format!(
		"INSERT INTO {} (name, value, error_count, created_at)
		 VALUES ('combo', 95.0, 100, NOW())",
		table_name
	);
	client.execute(&insert_sql, &[]).await.unwrap();

	let yaml = format!(
		r#"
sql: "SELECT value, error_count, created_at FROM {} WHERE name = 'combo'"
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
"#,
		table_name
	);

	let mut alert: AlertDefinition = serde_yaml::from_str(&yaml).unwrap();
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
	let (pool, table_name) = setup_test_db("test_metrics_multi").await;

	// Insert test data with multiple fields
	let client = pool.get().await.unwrap();
	let insert_sql = format!(
		"INSERT INTO {} (name, value, error_count) VALUES ('multi', 95.0, 150)",
		table_name
	);
	client.execute(&insert_sql, &[]).await.unwrap();

	let yaml = format!(
		r#"
sql: "SELECT value as cpu, error_count as errors FROM {} WHERE name = 'multi'"
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
"#,
		table_name
	);

	let mut alert: AlertDefinition = serde_yaml::from_str(&yaml).unwrap();
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
	let update_sql1 = format!("UPDATE {} SET value = 40 WHERE name = 'multi'", table_name);
	client.execute(&update_sql1, &[]).await.unwrap();

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
	let update_sql2 = format!(
		"UPDATE {} SET error_count = 30 WHERE name = 'multi'",
		table_name
	);
	client.execute(&update_sql2, &[]).await.unwrap();

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
