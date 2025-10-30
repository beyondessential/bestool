use rustyline::history::History;

use super::*;
use crate::theme::Theme;

#[tokio::test]
async fn test_psql_config_creation() {
	let connection_string = "postgresql://localhost/test";
	let pool = crate::pool::create_pool(connection_string)
		.await
		.expect("Failed to create pool");

	let config = PsqlConfig {
		pool,
		user: Some("testuser".to_string()),
		theme: Theme::Dark,
		audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
		database_name: "test".to_string(),
		write: false,
		use_colours: true,
	};

	assert_eq!(config.user, Some("testuser".to_string()));
	assert_eq!(config.database_name, "test");
}

#[tokio::test]
async fn test_psql_config_no_user() {
	let connection_string = "postgresql://localhost/test";
	let pool = crate::pool::create_pool(connection_string)
		.await
		.expect("Failed to create pool");

	let config = PsqlConfig {
		pool,
		user: None,
		theme: Theme::Dark,
		audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
		database_name: "test".to_string(),
		write: false,
		use_colours: true,
	};

	assert_eq!(config.user, None);
}

#[test]
fn test_psql_error_display() {
	let err = crate::config::PsqlError::ConnectionFailed;
	assert_eq!(format!("{err}"), "database connection failed");

	let err = crate::config::PsqlError::QueryFailed;
	assert_eq!(format!("{err}"), "query execution failed");
}

#[test]
fn test_snippet_save_excluded_from_preceding_command() {
	use crate::audit::Audit;
	use tempfile::TempDir;

	let temp_dir = TempDir::new().unwrap();
	let audit_path = temp_dir.path().join("history.redb");

	let repl_state = ReplState::new();
	let mut audit = Audit::open(&audit_path, repl_state).unwrap();
	audit.add_entry("SELECT 1;".into()).unwrap();
	audit.add_entry("SELECT 2;".into()).unwrap();

	let last_idx = audit.len() - 1;
	let last_entry = audit
		.get(last_idx, rustyline::history::SearchDirection::Forward)
		.unwrap();
	assert!(last_entry.is_some());
	if let Some(result) = last_entry {
		assert_eq!(result.entry, "SELECT 2;");
	}
}

#[tokio::test]
async fn test_text_cast_for_record_types() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	let mut stdout = tokio::io::stdout();
	let mut query_ctx = crate::query::QueryContext {
		client: &*client,
		modifiers: crate::parser::QueryModifiers::new(),
		theme: crate::theme::Theme::Dark,
		writer: &mut stdout,
		use_colours: true,
		vars: None,
	};
	let result =
		crate::query::execute_query("SELECT row(1, 'foo', true) as record", &mut query_ctx).await;

	assert!(result.is_ok());
}

#[tokio::test]
async fn test_array_formatting() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	let mut stdout = tokio::io::stdout();
	let mut query_ctx = crate::query::QueryContext {
		client: &*client,
		modifiers: crate::parser::QueryModifiers::new(),
		theme: crate::theme::Theme::Dark,
		writer: &mut stdout,
		use_colours: true,
		vars: None,
	};
	let result =
		crate::query::execute_query("SELECT ARRAY[1, 2, 3] as numbers", &mut query_ctx).await;

	assert!(result.is_ok());
}

#[tokio::test]
async fn test_database_info_query() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	let info_rows = client
			.query(
				"SELECT current_database(), current_user, usesuper FROM pg_user WHERE usename = current_user",
				&[],
			)
			.await
			.expect("Failed to query database info");

	assert!(!info_rows.is_empty());
	let row = info_rows.first().expect("No rows returned");
	let db_name: String = row.get(0);
	let _username: String = row.get(1);
	let _is_super: bool = row.get(2);

	assert!(!db_name.is_empty());
}

#[tokio::test]
async fn test_transaction_state_none() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");

	let state = TransactionState::check(&*monitor_client, backend_pid).await;

	assert_eq!(state, TransactionState::None);
}

#[tokio::test]
async fn test_transaction_state_idle() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");

	client
		.batch_execute("BEGIN")
		.await
		.expect("Failed to begin transaction");

	let state = TransactionState::check(&*monitor_client, backend_pid).await;
	assert_eq!(state, TransactionState::Idle);

	client.batch_execute("ROLLBACK").await.ok();
}

#[tokio::test]
async fn test_transaction_state_active() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");

	client
		.batch_execute("BEGIN; CREATE TEMP TABLE test_xid (id INT)")
		.await
		.expect("Failed to begin transaction and allocate XID");

	tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

	let state = TransactionState::check(&*monitor_client, backend_pid).await;
	assert_eq!(state, TransactionState::Active);

	client.batch_execute("ROLLBACK").await.ok();
}

#[tokio::test]
async fn test_transaction_state_error() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");

	client
		.batch_execute("BEGIN")
		.await
		.expect("Failed to begin transaction");

	let _ = client.query("SELECT 1/0", &[]).await;

	tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

	let state = TransactionState::check(&*monitor_client, backend_pid).await;
	assert_eq!(state, TransactionState::Error);

	client.batch_execute("ROLLBACK").await.ok();
}

#[tokio::test]
async fn test_write_mode_disable_with_idle_transaction() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");

	client
		.batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; BEGIN")
		.await
		.expect("Failed to enable write mode");

	let state = TransactionState::check(&monitor_client, backend_pid).await;
	assert_eq!(state, TransactionState::Idle);

	client
		.batch_execute("ROLLBACK; SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
		.await
		.expect("Failed to disable write mode with idle transaction");

	let state = TransactionState::check(&monitor_client, backend_pid).await;
	assert_eq!(state, TransactionState::None);
}

#[tokio::test]
async fn test_write_mode_disable_blocked_with_active_transaction() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");

	client
			.batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; BEGIN; CREATE TEMP TABLE test_write_block (id INT)")
			.await
			.expect("Failed to enable write mode and allocate XID");

	tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

	let state = TransactionState::check(&monitor_client, backend_pid).await;
	assert_eq!(state, TransactionState::Active);

	client.batch_execute("ROLLBACK").await.ok();
}

#[tokio::test]
async fn test_backend_xmin_vs_xid_in_idle_transaction() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute("BEGIN")
		.await
		.expect("Failed to begin transaction");

	let row = client
			.query_one(
				"SELECT backend_xid::text, backend_xmin::text FROM pg_stat_activity WHERE pid = pg_backend_pid()",
				&[],
			)
			.await
			.expect("Failed to query pg_stat_activity");

	let backend_xid: Option<String> = row.get(0);
	let backend_xmin: Option<String> = row.get(1);

	assert!(
		backend_xid.is_none() || backend_xid.as_ref().unwrap().is_empty(),
		"backend_xid should be NULL in idle transaction, got: {:?}",
		backend_xid
	);

	assert!(
		backend_xmin.is_some() && !backend_xmin.as_ref().unwrap().is_empty(),
		"backend_xmin should be set in idle transaction, got: {:?}",
		backend_xmin
	);

	client.batch_execute("ROLLBACK").await.ok();
}
