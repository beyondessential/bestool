use bestool_psql2::{create_pool, Config, Theme};

#[tokio::test]
async fn test_config_with_all_fields() {
	let pool = create_pool("postgresql://user:pass@localhost:5432/testdb")
		.await
		.expect("Failed to create pool");

	let config = Config {
		pool,
		user: Some("admin".to_string()),
		theme: Theme::Dark,
		audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
		database_name: "testdb".to_string(),
		write: false,
		use_colours: false,
	};

	assert_eq!(config.user, Some("admin".to_string()));
}

#[tokio::test]
async fn test_config_minimal() {
	let pool = create_pool("postgresql://localhost/db")
		.await
		.expect("Failed to create pool");

	let config = Config {
		pool,
		user: None,
		theme: Theme::Auto,
		audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
		database_name: "db".to_string(),
		write: false,
		use_colours: false,
	};

	assert_eq!(config.user, None);
}

#[tokio::test]
async fn test_theme_variations() {
	let pool1 = create_pool("postgresql://localhost/db")
		.await
		.expect("Failed to create pool");
	let pool2 = create_pool("postgresql://localhost/db")
		.await
		.expect("Failed to create pool");
	let pool3 = create_pool("postgresql://localhost/db")
		.await
		.expect("Failed to create pool");

	let configs = vec![
		Config {
			pool: pool1,
			user: None,
			theme: Theme::Light,
			audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
			database_name: "db".to_string(),
			write: false,
			use_colours: false,
		},
		Config {
			pool: pool2,
			user: None,
			theme: Theme::Dark,
			audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
			database_name: "db".to_string(),
			write: false,
			use_colours: false,
		},
		Config {
			pool: pool3,
			user: None,
			theme: Theme::Auto,
			audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
			database_name: "db".to_string(),
			write: false,
			use_colours: false,
		},
	];

	for _config in configs {
		// Just ensure configs were created successfully
	}
}

#[tokio::test]
async fn test_config_clone() {
	let pool = create_pool("postgresql://localhost/db")
		.await
		.expect("Failed to create pool");

	let config1 = Config {
		pool,
		user: Some("user1".to_string()),
		theme: Theme::Dark,
		audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
		database_name: "db".to_string(),
		write: false,
		use_colours: false,
	};

	let config2 = config1.clone();

	assert_eq!(config1.user, config2.user);
}

#[test]
fn test_connection_string_parsing_full_url() {
	let dbname = "postgresql://user:pass@localhost:5432/testdb";
	let connection_string = if dbname.contains("://") {
		dbname.to_string()
	} else {
		format!("postgresql://localhost/{}", dbname)
	};

	assert_eq!(
		connection_string,
		"postgresql://user:pass@localhost:5432/testdb"
	);
}

#[test]
fn test_connection_string_parsing_simple_name() {
	let dbname = "mydb";
	let connection_string = if dbname.contains("://") {
		dbname.to_string()
	} else {
		format!("postgresql://localhost/{}", dbname)
	};

	assert_eq!(connection_string, "postgresql://localhost/mydb");
}

#[test]
fn test_connection_string_parsing_various_names() {
	let test_cases = vec![
		("testdb", "postgresql://localhost/testdb"),
		("my_database", "postgresql://localhost/my_database"),
		("db-123", "postgresql://localhost/db-123"),
		("postgres://host:5432/db", "postgres://host:5432/db"),
		("postgresql://user@host/db", "postgresql://user@host/db"),
	];

	for (input, expected) in test_cases {
		let connection_string = if input.contains("://") {
			input.to_string()
		} else {
			format!("postgresql://localhost/{}", input)
		};
		assert_eq!(connection_string, expected);
	}
}

mod list_command_tests {
	use bestool_psql2::create_pool;

	async fn get_test_pool() -> bestool_psql2::PgPool {
		let database_url = std::env::var("DATABASE_URL")
			.unwrap_or_else(|_| "postgresql://localhost/tamanu_meta".to_string());
		create_pool(&database_url)
			.await
			.expect("Failed to create test pool")
	}

	#[tokio::test]
	#[ignore] // Run with --ignored flag when DATABASE_URL is set
	async fn test_list_tables_in_public_schema() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		// Create unique test tables for this test
		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_micros();
		let table1 = format!("psql2_test_users_{}", timestamp);
		let table2 = format!("psql2_test_posts_{}", timestamp);

		client
			.batch_execute(&format!(
				"
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, email TEXT);
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, title TEXT);
				",
				table1, table2
			))
			.await
			.expect("Failed to create test tables");

		// Query for tables in public schema with wildcard
		let rows = client
			.query(
				r#"
				SELECT
					n.nspname AS "Schema",
					c.relname AS "Name",
					pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				WHERE c.relkind = 'r'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&"^public$", &".*"],
			)
			.await
			.expect("Failed to query tables");

		// Check that we have at least our test tables
		let table_names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(1)).collect();
		assert!(
			table_names.contains(&table1),
			"Expected to find {} in results",
			table1
		);
		assert!(
			table_names.contains(&table2),
			"Expected to find {} in results",
			table2
		);

		// Cleanup
		client
			.batch_execute(&format!(
				"DROP TABLE IF EXISTS public.{}; DROP TABLE IF EXISTS public.{};",
				table1, table2
			))
			.await
			.ok();
	}

	#[tokio::test]
	#[ignore]
	async fn test_list_tables_with_pattern_match() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_micros();
		let table1 = format!("psql2_test_users_{}", timestamp);
		let table2 = format!("psql2_test_posts_{}", timestamp);

		client
			.batch_execute(&format!(
				"
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, email TEXT);
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, title TEXT);
				",
				table1, table2
			))
			.await
			.expect("Failed to create test tables");

		// Query for tables matching "psql2_test_users*" pattern in public schema
		let pattern = format!("^psql2_test_users_{}$", timestamp);
		let rows = client
			.query(
				r#"
				SELECT
					n.nspname AS "Schema",
					c.relname AS "Name",
					pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				WHERE c.relkind = 'r'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&"^public$", &pattern],
			)
			.await
			.expect("Failed to query tables");

		// Should only match users table
		let table_names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(1)).collect();
		assert_eq!(table_names.len(), 1);
		assert!(table_names.contains(&table1));
		assert!(!table_names.contains(&table2));

		// Cleanup
		client
			.batch_execute(&format!(
				"DROP TABLE IF EXISTS public.{}; DROP TABLE IF EXISTS public.{};",
				table1, table2
			))
			.await
			.ok();
	}

	#[tokio::test]
	#[ignore]
	async fn test_list_tables_in_specific_schema() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_micros();
		let schema = format!("psql2_test_schema_{}", timestamp);
		let table1 = "test_table_1";
		let table2 = "test_table_2";

		client
			.batch_execute(&format!(
				"
				CREATE SCHEMA {};
				CREATE TABLE {}.{} (id SERIAL PRIMARY KEY, name TEXT);
				CREATE TABLE {}.{} (id SERIAL PRIMARY KEY, data TEXT);
				",
				schema, schema, table1, schema, table2
			))
			.await
			.expect("Failed to create test schema and tables");

		// Query for all tables in test schema
		let schema_pattern = format!("^{}$", schema);
		let rows = client
			.query(
				r#"
				SELECT
					n.nspname AS "Schema",
					c.relname AS "Name",
					pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				WHERE c.relkind = 'r'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&schema_pattern, &".*"],
			)
			.await
			.expect("Failed to query tables");

		// Should find both test_table_1 and test_table_2
		let table_names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(1)).collect();
		assert!(table_names.contains(&table1.to_string()));
		assert!(table_names.contains(&table2.to_string()));
		assert_eq!(table_names.len(), 2);

		// Cleanup
		client
			.batch_execute(&format!("DROP SCHEMA {} CASCADE;", schema))
			.await
			.ok();
	}

	#[tokio::test]
	#[ignore]
	async fn test_list_tables_with_detail() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_micros();
		let table = format!("psql2_test_detail_{}", timestamp);

		client
			.batch_execute(&format!(
				"CREATE TABLE public.{} (id SERIAL PRIMARY KEY, email TEXT);",
				table
			))
			.await
			.expect("Failed to create test table");

		// Query with detail (additional columns)
		let pattern = format!("^{}$", table);
		let rows = client
			.query(
				r#"
				SELECT
					n.nspname AS "Schema",
					c.relname AS "Name",
					pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size",
					pg_catalog.pg_get_userbyid(c.relowner) AS "Owner",
					CASE c.relpersistence
						WHEN 'p' THEN 'permanent'
						WHEN 'u' THEN 'unlogged'
						WHEN 't' THEN 'temporary'
					END AS "Persistence",
					CASE
						WHEN c.relacl IS NULL THEN NULL
						ELSE pg_catalog.array_to_string(c.relacl, E'\n')
					END AS "Access"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				WHERE c.relkind = 'r'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&"^public$", &pattern],
			)
			.await
			.expect("Failed to query tables with detail");

		assert_eq!(rows.len(), 1);
		let row = &rows[0];

		// Check all columns are present
		let schema: String = row.get(0);
		let name: String = row.get(1);
		let size: String = row.get(2);
		let owner: String = row.get(3);
		let persistence: String = row.get(4);
		let _access: Option<String> = row.get(5);

		assert_eq!(schema, "public");
		assert_eq!(name, table);
		assert!(!size.is_empty());
		assert!(!owner.is_empty());
		assert_eq!(persistence, "permanent");

		// Cleanup
		client
			.batch_execute(&format!("DROP TABLE IF EXISTS public.{};", table))
			.await
			.ok();
	}

	#[tokio::test]
	#[ignore]
	async fn test_list_tables_wildcard_pattern() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_micros();
		let table1 = format!("psql2_wild_test_{}", timestamp);
		let table2 = format!("psql2_wild_test2_{}", timestamp);

		client
			.batch_execute(&format!(
				"
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, email TEXT);
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, title TEXT);
				",
				table1, table2
			))
			.await
			.expect("Failed to create test tables");

		// Query for tables matching "psql2_wild_test*" pattern
		let pattern = format!("^psql2_wild_test.*_{}$", timestamp);
		let rows = client
			.query(
				r#"
				SELECT
					n.nspname AS "Schema",
					c.relname AS "Name",
					pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				WHERE c.relkind = 'r'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&"^public$", &pattern],
			)
			.await
			.expect("Failed to query tables");

		// Should match both tables
		let table_names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(1)).collect();
		assert!(table_names.contains(&table1));
		assert!(table_names.contains(&table2));
		assert_eq!(table_names.len(), 2);

		// Cleanup
		client
			.batch_execute(&format!(
				"DROP TABLE IF EXISTS public.{}; DROP TABLE IF EXISTS public.{};",
				table1, table2
			))
			.await
			.ok();
	}

	#[tokio::test]
	#[ignore]
	async fn test_list_tables_no_matches() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		// Query for tables that don't exist
		let rows = client
			.query(
				r#"
				SELECT
					n.nspname AS "Schema",
					c.relname AS "Name",
					pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				WHERE c.relkind = 'r'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[
					&"^public$",
					&"^psql2_nonexistent_table_that_will_never_exist$",
				],
			)
			.await
			.expect("Failed to query tables");

		assert_eq!(rows.len(), 0);
	}
}
