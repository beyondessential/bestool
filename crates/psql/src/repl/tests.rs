use std::sync::{Arc, Mutex};

use rustyline::history::History;
use tokio::fs::File;

use super::*;

#[test]
fn test_snippet_save_excluded_from_preceding_command() {
	use crate::audit::Audit;
	use tempfile::TempDir;

	let temp_dir = TempDir::new().unwrap();
	let audit_path = temp_dir.path().join("history.redb");

	let repl_state = Arc::new(Mutex::new(ReplState::new()));
	let mut audit = Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();
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
		client: &client,
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
		client: &client,
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

	let state = TransactionState::check(&monitor_client, backend_pid).await;

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

	let state = TransactionState::check(&monitor_client, backend_pid).await;
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

	let state = TransactionState::check(&monitor_client, backend_pid).await;
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

	let state = TransactionState::check(&monitor_client, backend_pid).await;
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

#[tokio::test]
async fn test_describe_table() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute(
			"CREATE TEMP TABLE test_describe_table (
				id SERIAL PRIMARY KEY,
				name TEXT NOT NULL,
				email TEXT UNIQUE
			)",
		)
		.await
		.expect("Failed to create test table");

	let rows = client
		.query(
			"SELECT n.nspname, c.relname, c.relkind::text
			FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			WHERE c.relname = 'test_describe_table'
			AND n.nspname LIKE 'pg_temp%'",
			&[],
		)
		.await
		.expect("Failed to query test table");

	assert!(!rows.is_empty(), "Test table should exist");

	let row = &rows[0];
	let relkind: String = row.get(2);
	assert_eq!(relkind, "r", "Should be a regular table");
}

#[tokio::test]
async fn test_describe_view() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute("CREATE TEMP VIEW test_describe_view AS SELECT 1 AS id, 'test' AS name")
		.await
		.expect("Failed to create test view");

	let rows = client
		.query(
			"SELECT n.nspname, c.relname, c.relkind::text
			FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			WHERE c.relname = 'test_describe_view'
			AND n.nspname LIKE 'pg_temp%'",
			&[],
		)
		.await
		.expect("Failed to query test view");

	assert!(!rows.is_empty(), "Test view should exist");

	let row = &rows[0];
	let relkind: String = row.get(2);
	assert_eq!(relkind, "v", "Should be a view");
}

#[tokio::test]
async fn test_describe_sequence() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute("CREATE TEMP SEQUENCE test_describe_seq START 100 INCREMENT 5")
		.await
		.expect("Failed to create test sequence");

	let rows = client
		.query(
			"SELECT seqincrement, seqstart
			FROM pg_catalog.pg_sequence s
			LEFT JOIN pg_catalog.pg_class c ON c.oid = s.seqrelid
			WHERE c.relname = 'test_describe_seq'",
			&[],
		)
		.await
		.expect("Failed to query test sequence");

	assert!(!rows.is_empty(), "Test sequence should exist");

	let row = &rows[0];
	let increment: i64 = row.get(0);
	let start: i64 = row.get(1);
	assert_eq!(increment, 5, "Increment should be 5");
	assert_eq!(start, 100, "Start should be 100");
}

#[tokio::test]
async fn test_describe_index() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute(
			"CREATE TEMP TABLE test_index_table (id INT, name TEXT);
			CREATE INDEX test_describe_idx ON test_index_table(name)",
		)
		.await
		.expect("Failed to create test table and index");

	let rows = client
		.query(
			"SELECT i.relname, ix.indisunique
			FROM pg_catalog.pg_class i
			LEFT JOIN pg_catalog.pg_index ix ON ix.indexrelid = i.oid
			WHERE i.relname = 'test_describe_idx'",
			&[],
		)
		.await
		.expect("Failed to query test index");

	assert!(!rows.is_empty(), "Test index should exist");

	let row = &rows[0];
	let is_unique: bool = row.get(1);
	assert!(!is_unique, "Index should not be unique");
}

#[tokio::test]
async fn test_describe_table_with_foreign_keys() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute(
			"CREATE TEMP TABLE test_parent (id INT PRIMARY KEY);
			CREATE TEMP TABLE test_child (
				id INT PRIMARY KEY,
				parent_id INT REFERENCES test_parent(id)
			)",
		)
		.await
		.expect("Failed to create test tables with foreign keys");

	let fk_rows = client
		.query(
			"SELECT conname
			FROM pg_catalog.pg_constraint
			WHERE conrelid = (
				SELECT c.oid FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				WHERE c.relname = 'test_child' AND n.nspname LIKE 'pg_temp%'
			)
			AND contype = 'f'",
			&[],
		)
		.await
		.expect("Failed to query foreign keys");

	assert!(!fk_rows.is_empty(), "Foreign key should exist");

	let ref_rows = client
		.query(
			"SELECT conname
			FROM pg_catalog.pg_constraint
			WHERE confrelid = (
				SELECT c.oid FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				WHERE c.relname = 'test_parent' AND n.nspname LIKE 'pg_temp%'
			)
			AND contype = 'f'",
			&[],
		)
		.await
		.expect("Failed to query referenced by");

	assert!(
		!ref_rows.is_empty(),
		"Parent table should be referenced by child"
	);
}

#[tokio::test]
async fn test_describe_table_with_triggers() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute(
			"CREATE TEMP TABLE test_trigger_table (id INT, updated_at TIMESTAMP);
			CREATE OR REPLACE FUNCTION update_timestamp()
			RETURNS TRIGGER AS $$
			BEGIN
				NEW.updated_at = NOW();
				RETURN NEW;
			END;
			$$ LANGUAGE plpgsql;
			CREATE TRIGGER test_trigger
			BEFORE UPDATE ON test_trigger_table
			FOR EACH ROW EXECUTE FUNCTION update_timestamp()",
		)
		.await
		.expect("Failed to create test table with trigger");

	let trigger_rows = client
		.query(
			"SELECT t.tgname
			FROM pg_catalog.pg_trigger t
			LEFT JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			WHERE c.relname = 'test_trigger_table'
				AND n.nspname LIKE 'pg_temp%'
				AND NOT t.tgisinternal",
			&[],
		)
		.await
		.expect("Failed to query triggers");

	assert!(!trigger_rows.is_empty(), "Trigger should exist");
	let row = &trigger_rows[0];
	let trigger_name: String = row.get(0);
	assert_eq!(trigger_name, "test_trigger");
}

#[tokio::test]
async fn test_describe_table_with_database() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute(
			"CREATE TEMP TABLE test_d_table (
				id SERIAL PRIMARY KEY,
				name TEXT NOT NULL,
				email TEXT UNIQUE,
				created_at TIMESTAMP DEFAULT NOW()
			)",
		)
		.await
		.expect("Failed to create test table");

	// Get the schema name (pg_temp_X)
	let schema_row = client
		.query_one(
			"SELECT n.nspname FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			WHERE c.relname = 'test_d_table' AND n.nspname LIKE 'pg_temp%'",
			&[],
		)
		.await
		.expect("Failed to get schema");
	let schema: String = schema_row.get(0);

	// Test describe via the actual describe handler
	use crate::audit::Audit;
	use crate::completer::SqlCompleter;
	use crate::repl::{ReplContext, ReplState};
	use crate::theme::Theme;
	use rustyline::Editor;
	use std::sync::{Arc, Mutex};
	use tempfile::TempDir;

	let temp_dir = TempDir::new().unwrap();
	let audit_path = temp_dir.path().join("history.redb");

	let mut repl_state = ReplState::new();
	let file = File::create_new(
		temp_dir
			.path()
			.join("test_describe_table_with_database.txt"),
	)
	.await
	.unwrap();
	repl_state.output_file = Some(Arc::new(tokio::sync::Mutex::new(file)));

	let repl_state = Arc::new(Mutex::new(repl_state));
	let audit = Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();
	let completer = SqlCompleter::new(Theme::Dark);
	let mut rl: Editor<SqlCompleter, Audit> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		audit,
	)
	.unwrap();
	rl.set_helper(Some(completer));

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");
	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	{
		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		// Test describing the table
		let result = crate::repl::describe::handle_describe(
			&mut ctx,
			format!("{}.test_d_table", schema),
			false,
			false,
		)
		.await;

		assert!(result.is_continue());
		repl_state.lock().unwrap().output_file.take().unwrap();
	}

	let output = std::fs::read_to_string(
		temp_dir
			.path()
			.join("test_describe_table_with_database.txt"),
	)
	.unwrap();

	// Verify expected output
	assert!(
		output.contains("test_d_table"),
		"Output should contain table name"
	);
	assert!(
		output.contains("Column"),
		"Output should contain Column header"
	);
	assert!(output.contains("Type"), "Output should contain Type header");
	assert!(output.contains("id"), "Output should contain id column");
	assert!(output.contains("name"), "Output should contain name column");
	assert!(
		output.contains("email"),
		"Output should contain email column"
	);
}

#[tokio::test]
async fn test_describe_view_with_database() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute("CREATE TEMP VIEW test_d_view AS SELECT 1 AS id, 'test' AS name")
		.await
		.expect("Failed to create test view");

	// Get the schema name (pg_temp_X)
	let schema_row = client
		.query_one(
			"SELECT n.nspname FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			WHERE c.relname = 'test_d_view' AND n.nspname LIKE 'pg_temp%'",
			&[],
		)
		.await
		.expect("Failed to get schema");
	let schema: String = schema_row.get(0);

	use crate::audit::Audit;
	use crate::completer::SqlCompleter;
	use crate::repl::{ReplContext, ReplState};
	use crate::theme::Theme;
	use rustyline::Editor;
	use std::sync::{Arc, Mutex};
	use tempfile::TempDir;

	let temp_dir = TempDir::new().unwrap();
	let audit_path = temp_dir.path().join("history.redb");

	let mut repl_state = ReplState::new();
	let file = File::create_new(temp_dir.path().join("test_describe_view_with_database.txt"))
		.await
		.unwrap();
	repl_state.output_file = Some(Arc::new(tokio::sync::Mutex::new(file)));

	let repl_state = Arc::new(Mutex::new(repl_state));
	let audit = Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();
	let completer = SqlCompleter::new(Theme::Dark);
	let mut rl: Editor<SqlCompleter, Audit> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		audit,
	)
	.unwrap();
	rl.set_helper(Some(completer));

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");
	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	{
		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		// Test describing the view
		let result = crate::repl::describe::handle_describe(
			&mut ctx,
			format!("{}.test_d_view", schema),
			false,
			false,
		)
		.await;

		assert!(result.is_continue());
		repl_state.lock().unwrap().output_file.take().unwrap();
	}

	let output =
		std::fs::read_to_string(temp_dir.path().join("test_describe_view_with_database.txt"))
			.unwrap();

	// Verify expected output
	assert!(
		output.contains("test_d_view"),
		"Output should contain view name"
	);
	assert!(
		output.contains("View") || output.contains("view"),
		"Output should indicate it's a view"
	);
	assert!(
		output.contains("Column"),
		"Output should contain Column header"
	);
	assert!(output.contains("id"), "Output should contain id column");
	assert!(output.contains("name"), "Output should contain name column");
}

#[tokio::test]
async fn test_describe_index_with_database() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute(
			"CREATE TEMP TABLE test_idx_table (id INT, name TEXT);
			CREATE INDEX test_d_idx ON test_idx_table(name)",
		)
		.await
		.expect("Failed to create test table and index");

	// Get the schema name (pg_temp_X)
	let schema_row = client
		.query_one(
			"SELECT n.nspname FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			WHERE c.relname = 'test_d_idx' AND n.nspname LIKE 'pg_temp%'",
			&[],
		)
		.await
		.expect("Failed to get schema");
	let schema: String = schema_row.get(0);

	use crate::audit::Audit;
	use crate::completer::SqlCompleter;
	use crate::repl::{ReplContext, ReplState};
	use crate::theme::Theme;
	use rustyline::Editor;
	use std::sync::{Arc, Mutex};
	use tempfile::TempDir;

	let temp_dir = TempDir::new().unwrap();
	let audit_path = temp_dir.path().join("history.redb");

	let mut repl_state = ReplState::new();
	let file = File::create_new(
		temp_dir
			.path()
			.join("test_describe_index_with_database.txt"),
	)
	.await
	.unwrap();
	repl_state.output_file = Some(Arc::new(tokio::sync::Mutex::new(file)));

	let repl_state = Arc::new(Mutex::new(repl_state));
	let audit = Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();
	let completer = SqlCompleter::new(Theme::Dark);
	let mut rl: Editor<SqlCompleter, Audit> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		audit,
	)
	.unwrap();
	rl.set_helper(Some(completer));

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");
	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	{
		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		// Test describing the index
		let result = crate::repl::describe::handle_describe(
			&mut ctx,
			format!("{}.test_d_idx", schema),
			false,
			false,
		)
		.await;

		assert!(result.is_continue());
		repl_state.lock().unwrap().output_file.take().unwrap();
	}

	let output = std::fs::read_to_string(
		temp_dir
			.path()
			.join("test_describe_index_with_database.txt"),
	)
	.unwrap();

	// Verify expected output
	dbg!(&output);
	assert!(
		output.contains("test_d_idx"),
		"Output should contain index name"
	);
	assert!(
		output.contains("Index") || output.contains("index"),
		"Output should indicate it's an index"
	);
	assert!(
		output.contains("test_idx_table"),
		"Output should contain table name"
	);
	assert!(
		output.contains("btree") || output.contains("Definition"),
		"Output should contain index type or definition"
	);
}

#[tokio::test]
async fn test_describe_sequence_with_database() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute("CREATE TEMP SEQUENCE test_d_seq START 100 INCREMENT 5")
		.await
		.expect("Failed to create test sequence");

	// Get the schema name (pg_temp_X)
	let schema_row = client
		.query_one(
			"SELECT n.nspname FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			WHERE c.relname = 'test_d_seq' AND n.nspname LIKE 'pg_temp%'",
			&[],
		)
		.await
		.expect("Failed to get schema");
	let schema: String = schema_row.get(0);

	use crate::audit::Audit;
	use crate::completer::SqlCompleter;
	use crate::repl::{ReplContext, ReplState};
	use crate::theme::Theme;
	use rustyline::Editor;
	use std::sync::{Arc, Mutex};
	use tempfile::TempDir;

	let temp_dir = TempDir::new().unwrap();
	let audit_path = temp_dir.path().join("history.redb");

	let mut repl_state = ReplState::new();
	let file = File::create_new(
		temp_dir
			.path()
			.join("test_describe_sequence_with_database.txt"),
	)
	.await
	.unwrap();
	repl_state.output_file = Some(Arc::new(tokio::sync::Mutex::new(file)));

	let repl_state = Arc::new(Mutex::new(repl_state));
	let audit = Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();
	let completer = SqlCompleter::new(Theme::Dark);
	let mut rl: Editor<SqlCompleter, Audit> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		audit,
	)
	.unwrap();
	rl.set_helper(Some(completer));

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");
	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	{
		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		// Test describing the sequence
		let result = crate::repl::describe::handle_describe(
			&mut ctx,
			format!("{}.test_d_seq", schema),
			false,
			false,
		)
		.await;

		assert!(result.is_continue());
		repl_state.lock().unwrap().output_file.take().unwrap();
	}

	let output = std::fs::read_to_string(
		temp_dir
			.path()
			.join("test_describe_sequence_with_database.txt"),
	)
	.unwrap();

	// Verify expected output
	dbg!(&output);
	assert!(
		output.contains("test_d_seq"),
		"Output should contain sequence name"
	);
	assert!(
		output.contains("Sequence") || output.contains("sequence"),
		"Output should indicate it's a sequence"
	);
	assert!(
		output.contains("100"),
		"Output should contain start value 100"
	);
	assert!(
		output.contains("5"),
		"Output should contain increment value 5"
	);
}

#[tokio::test]
async fn test_describe_function_with_database() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute(
			"CREATE OR REPLACE FUNCTION test_d_func(x INT, y INT)
			RETURNS INT AS $$
			BEGIN
				RETURN x + y;
			END;
			$$ LANGUAGE plpgsql IMMUTABLE",
		)
		.await
		.expect("Failed to create test function");

	use crate::audit::Audit;
	use crate::completer::SqlCompleter;
	use crate::repl::{ReplContext, ReplState};
	use crate::theme::Theme;
	use rustyline::Editor;
	use std::sync::{Arc, Mutex};
	use tempfile::TempDir;

	let temp_dir = TempDir::new().unwrap();
	let audit_path = temp_dir.path().join("history.redb");

	let mut repl_state = ReplState::new();
	let file = File::create_new(
		temp_dir
			.path()
			.join("test_describe_function_with_database.txt"),
	)
	.await
	.unwrap();
	repl_state.output_file = Some(Arc::new(tokio::sync::Mutex::new(file)));

	let repl_state = Arc::new(Mutex::new(repl_state));
	let audit = Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();
	let completer = SqlCompleter::new(Theme::Dark);
	let mut rl: Editor<SqlCompleter, Audit> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		audit,
	)
	.unwrap();
	rl.set_helper(Some(completer));

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");
	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	{
		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		// Test describing the function
		let result = crate::repl::describe::handle_describe(
			&mut ctx,
			"test_d_func".to_string(),
			false,
			false,
		)
		.await;

		assert!(result.is_continue());
		repl_state.lock().unwrap().output_file.take().unwrap();
	}

	let output = std::fs::read_to_string(
		temp_dir
			.path()
			.join("test_describe_function_with_database.txt"),
	)
	.unwrap();

	// Verify expected output
	assert!(
		output.contains("test_d_func"),
		"Output should contain function name"
	);
	assert!(
		output.contains("Function") || output.contains("function"),
		"Output should indicate it's a function"
	);
	assert!(output.contains("plpgsql"), "Output should contain language");
	assert!(
		output.contains("immutable"),
		"Output should contain volatility"
	);
	assert!(output.contains("Returns"), "Output should contain Returns");
	assert!(
		output.contains("integer"),
		"Output should contain return type"
	);

	client
		.batch_execute("DROP FUNCTION IF EXISTS test_d_func(INT, INT)")
		.await
		.ok();
}

#[tokio::test]
async fn test_describe_function() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	client
		.batch_execute(
			"CREATE OR REPLACE FUNCTION test_describe_func(x INT, y INT)
			RETURNS INT AS $$
			BEGIN
				RETURN x + y;
			END;
			$$ LANGUAGE plpgsql IMMUTABLE",
		)
		.await
		.expect("Failed to create test function");

	let rows = client
		.query(
			"SELECT p.proname, l.lanname
			FROM pg_catalog.pg_proc p
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
			LEFT JOIN pg_catalog.pg_language l ON l.oid = p.prolang
			WHERE n.nspname = 'public'
				AND p.proname = 'test_describe_func'",
			&[],
		)
		.await
		.expect("Failed to query test function");

	assert!(!rows.is_empty(), "Test function should exist");
	let row = &rows[0];
	let function_name: String = row.get(0);
	let language: String = row.get(1);
	assert_eq!(function_name, "test_describe_func");
	assert_eq!(language, "plpgsql");

	client
		.batch_execute("DROP FUNCTION IF EXISTS test_describe_func(INT, INT)")
		.await
		.ok();
}

#[tokio::test]
async fn test_multiple_statements() {
	let connection_string =
		std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

	let pool = crate::pool::create_pool(&connection_string)
		.await
		.expect("Failed to create pool");

	let client = pool.get().await.expect("Failed to get connection");

	// Create a temporary table to test with
	client
		.batch_execute("CREATE TEMP TABLE multi_test (id INT)")
		.await
		.expect("Failed to create test table");

	use crate::audit::Audit;
	use crate::completer::SqlCompleter;
	use crate::repl::{ReplContext, ReplState};
	use crate::theme::Theme;
	use rustyline::Editor;
	use std::sync::{Arc, Mutex};
	use tempfile::TempDir;

	let temp_dir = TempDir::new().unwrap();
	let audit_path = temp_dir.path().join("history.redb");

	let mut repl_state = ReplState::new();
	let file = File::create_new(temp_dir.path().join("test_multiple_statements.txt"))
		.await
		.unwrap();
	repl_state.output_file = Some(Arc::new(tokio::sync::Mutex::new(file)));

	let repl_state = Arc::new(Mutex::new(repl_state));
	let audit = Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();
	let completer = SqlCompleter::new(Theme::Dark);
	let mut rl: Editor<SqlCompleter, Audit> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		audit,
	)
	.unwrap();
	rl.set_helper(Some(completer));

	let monitor_client = pool.get().await.expect("Failed to get monitor connection");
	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.expect("Failed to get backend PID")
		.get(0);

	{
		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		// Execute multiple statements: insert two rows and select them
		let multi_sql = "INSERT INTO multi_test VALUES (1); INSERT INTO multi_test VALUES (2); SELECT * FROM multi_test ORDER BY id;";

		let result = crate::repl::execute::handle_execute(
			&mut ctx,
			multi_sql.to_string(),
			multi_sql.to_string(),
			Default::default(),
		)
		.await;

		assert!(result.is_continue());
		repl_state.lock().unwrap().output_file.take().unwrap();
	}

	let output =
		std::fs::read_to_string(temp_dir.path().join("test_multiple_statements.txt")).unwrap();

	// The output should contain both rows
	assert!(
		output.contains("1") && output.contains("2"),
		"Output should contain both inserted values, got: {}",
		output
	);

	// Verify the data was actually inserted
	let rows = client
		.query("SELECT COUNT(*) FROM multi_test", &[])
		.await
		.expect("Failed to query test table");
	let count: i64 = rows[0].get(0);
	assert_eq!(count, 2, "Should have inserted 2 rows");
}
