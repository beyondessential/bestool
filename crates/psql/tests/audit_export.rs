use bestool_psql::{Audit, QueryOptions};

#[test]
fn test_audit_export_basic() {
	let temp_dir = tempfile::tempdir().unwrap();
	let db_path = temp_dir.path().join("test.redb");

	// Create a database with some entries
	{
		let db = redb::Database::create(&db_path).unwrap();
		let mut audit = Audit {
			db: std::sync::Arc::new(db),
			repl_state: std::sync::Arc::new(std::sync::Mutex::new(
				bestool_psql::repl::ReplState::new(),
			)),
			working_info: None,
			sync_thread: None,
		};

		for i in 0..10 {
			audit.add_entry(format!("SELECT {};", i)).unwrap();
			std::thread::sleep(std::time::Duration::from_micros(10));
		}
	}

	// Open and query the database
	let audit = Audit::open_file(&db_path).unwrap();

	// Test default query (last 100 entries, should return all 10)
	let options = QueryOptions::default();
	let entries = audit.query(&options).unwrap();
	assert_eq!(entries.len(), 10);

	// Verify they're in chronological order (oldest first)
	assert_eq!(entries[0].1.query, "SELECT 0;");
	assert_eq!(entries[9].1.query, "SELECT 9;");
}

#[test]
fn test_audit_export_limit() {
	let temp_dir = tempfile::tempdir().unwrap();
	let db_path = temp_dir.path().join("test.redb");

	// Create a database with some entries
	{
		let db = redb::Database::create(&db_path).unwrap();
		let mut audit = Audit {
			db: std::sync::Arc::new(db),
			repl_state: std::sync::Arc::new(std::sync::Mutex::new(
				bestool_psql::repl::ReplState::new(),
			)),
			working_info: None,
			sync_thread: None,
		};

		for i in 0..20 {
			audit.add_entry(format!("SELECT {};", i)).unwrap();
			std::thread::sleep(std::time::Duration::from_micros(10));
		}
	}

	// Open and query the database
	let audit = Audit::open_file(&db_path).unwrap();

	// Test limit of 5, from newest (default)
	let options = QueryOptions {
		limit: Some(5),
		from_oldest: false,
		..Default::default()
	};
	let entries = audit.query(&options).unwrap();
	assert_eq!(entries.len(), 5);
	assert_eq!(entries[0].1.query, "SELECT 15;");
	assert_eq!(entries[4].1.query, "SELECT 19;");

	// Test limit of 5, from oldest
	let options = QueryOptions {
		limit: Some(5),
		from_oldest: true,
		..Default::default()
	};
	let entries = audit.query(&options).unwrap();
	assert_eq!(entries.len(), 5);
	assert_eq!(entries[0].1.query, "SELECT 0;");
	assert_eq!(entries[4].1.query, "SELECT 4;");
}

#[test]
fn test_audit_export_json_serialization() {
	let temp_dir = tempfile::tempdir().unwrap();
	let db_path = temp_dir.path().join("test.redb");

	// Create a database with custom state
	{
		let db = redb::Database::create(&db_path).unwrap();
		let mut repl_state = bestool_psql::repl::ReplState::new();
		repl_state.db_user = "testdb".to_string();
		repl_state.sys_user = "testuser".to_string();
		repl_state.write_mode = true;
		repl_state.ots = Some("John Doe".to_string());

		let mut audit = Audit {
			db: std::sync::Arc::new(db),
			repl_state: std::sync::Arc::new(std::sync::Mutex::new(repl_state)),
			working_info: None,
			sync_thread: None,
		};

		audit
			.add_entry("INSERT INTO test VALUES (1);".to_string())
			.unwrap();
	}

	// Open and query the database
	let audit = Audit::open_file(&db_path).unwrap();

	let options = QueryOptions::default();
	let entries = audit.query(&options).unwrap();
	assert_eq!(entries.len(), 1);

	// Verify JSON serialization with timestamp
	let entry_with_ts = bestool_psql::AuditEntryWithTimestamp::from_entry_and_timestamp(
		entries[0].1.clone(),
		entries[0].0,
	);
	let json = serde_json::to_string(&entry_with_ts).unwrap();

	// Verify ts field is present and in RFC3339 format
	assert!(json.contains("\"ts\":"));
	assert!(json.contains("INSERT INTO test VALUES (1);"));
	assert!(json.contains("\"db_user\":\"testdb\""));
	assert!(json.contains("\"sys_user\":\"testuser\""));
	assert!(json.contains("John Doe"));

	// Verify it's valid compact JSON (no extra whitespace)
	assert!(!json.contains("\n"));

	// Verify ts can be parsed back
	let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
	assert!(parsed["ts"].is_string());
	let ts_str = parsed["ts"].as_str().unwrap();
	assert!(ts_str.parse::<jiff::Timestamp>().is_ok());
}

#[test]
fn test_audit_export_default_limit() {
	let temp_dir = tempfile::tempdir().unwrap();
	let db_path = temp_dir.path().join("test.redb");

	// Create a database with 150 entries
	{
		let db = redb::Database::create(&db_path).unwrap();
		let mut audit = Audit {
			db: std::sync::Arc::new(db),
			repl_state: std::sync::Arc::new(std::sync::Mutex::new(
				bestool_psql::repl::ReplState::new(),
			)),
			working_info: None,
			sync_thread: None,
		};

		for i in 0..150 {
			audit.add_entry(format!("SELECT {};", i)).unwrap();
			std::thread::sleep(std::time::Duration::from_micros(10));
		}
	}

	// Open and query with default options (limit should be 100)
	let audit = Audit::open_file(&db_path).unwrap();
	let options = QueryOptions {
		limit: Some(100),
		..Default::default()
	};
	let entries = audit.query(&options).unwrap();

	// Should return last 100 entries (newest), but in chronological order
	assert_eq!(entries.len(), 100);
	assert_eq!(entries[0].1.query, "SELECT 50;");
	assert_eq!(entries[99].1.query, "SELECT 149;");
}

#[test]
fn test_audit_export_unlimited() {
	let temp_dir = tempfile::tempdir().unwrap();
	let db_path = temp_dir.path().join("test.redb");

	// Create a database with 150 entries
	{
		let db = redb::Database::create(&db_path).unwrap();
		let mut audit = Audit {
			db: std::sync::Arc::new(db),
			repl_state: std::sync::Arc::new(std::sync::Mutex::new(
				bestool_psql::repl::ReplState::new(),
			)),
			working_info: None,
			sync_thread: None,
		};

		for i in 0..150 {
			audit.add_entry(format!("SELECT {};", i)).unwrap();
			std::thread::sleep(std::time::Duration::from_micros(10));
		}
	}

	// Open and query with limit=0 (should return all)
	let audit = Audit::open_file(&db_path).unwrap();
	let options = QueryOptions {
		limit: Some(0),
		..Default::default()
	};
	let entries = audit.query(&options).unwrap();

	// Should return all 150 entries
	assert_eq!(entries.len(), 150);
	assert_eq!(entries[0].1.query, "SELECT 0;");
	assert_eq!(entries[149].1.query, "SELECT 149;");
}

#[test]
fn test_audit_find_orphans() {
	let temp_dir = tempfile::tempdir().unwrap();

	// Create a fake orphan database
	let orphan_path = temp_dir.path().join("audit-working-test-orphan.redb");
	std::fs::write(&orphan_path, b"fake data").unwrap();

	// Set modification time to be old enough
	let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
	filetime::set_file_mtime(&orphan_path, filetime::FileTime::from_system_time(old_time)).unwrap();

	// Also need a main database file for the search to work
	let main_path = temp_dir.path().join("audit-main.redb");
	std::fs::write(&main_path, b"fake main").unwrap();

	// Find orphans
	let orphans = Audit::find_orphans(temp_dir.path()).unwrap();

	// Should find the old working database
	assert_eq!(orphans.len(), 1);
	assert!(
		orphans[0]
			.file_name()
			.unwrap()
			.to_str()
			.unwrap()
			.contains("working")
	);
}

#[test]
fn test_audit_open_file() {
	let temp_dir = tempfile::tempdir().unwrap();
	let db_path = temp_dir.path().join("test.redb");

	// Create a database with some entries
	{
		let db = redb::Database::create(&db_path).unwrap();
		let mut audit = Audit {
			db: std::sync::Arc::new(db),
			repl_state: std::sync::Arc::new(std::sync::Mutex::new(
				bestool_psql::repl::ReplState::new(),
			)),
			working_info: None,
			sync_thread: None,
		};

		audit.add_entry("SELECT 1;".to_string()).unwrap();
		audit.add_entry("SELECT 2;".to_string()).unwrap();
	}

	// Open it as a file
	let audit = Audit::open_file(&db_path).unwrap();
	let entries = audit.query(&QueryOptions::default()).unwrap();

	assert_eq!(entries.len(), 2);
	assert_eq!(entries[0].1.query, "SELECT 1;");
	assert_eq!(entries[1].1.query, "SELECT 2;");
}
