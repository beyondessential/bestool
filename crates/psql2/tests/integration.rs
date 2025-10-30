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
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
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
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
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
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
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
					am.amname AS "Access method",
					CASE
						WHEN c.relacl IS NULL THEN NULL
						ELSE pg_catalog.array_to_string(c.relacl, E'\n')
					END AS "ACL"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				LEFT JOIN pg_catalog.pg_am am ON c.relam = am.oid
				WHERE c.relkind = 'r'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
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
		let access_method: Option<String> = row.get(5);
		let _acl: Option<String> = row.get(6);

		assert_eq!(schema, "public");
		assert_eq!(name, table);
		assert!(!size.is_empty());
		assert!(!owner.is_empty());
		assert_eq!(persistence, "permanent");
		assert!(access_method.is_some(), "Access method should be present");

		// Cleanup
		client
			.batch_execute(&format!("DROP TABLE IF EXISTS public.{};", table))
			.await
			.ok();
	}

	#[tokio::test]
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
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
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
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
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

	#[tokio::test]
	async fn test_list_all_tables_with_star() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_micros();
		let schema = format!("psql2_test_star_{}", timestamp);
		let table1 = "test_table_1";
		let table2 = format!("psql2_star_test_{}", timestamp);

		client
			.batch_execute(&format!(
				"
				CREATE SCHEMA {};
				CREATE TABLE {}.{} (id SERIAL PRIMARY KEY, name TEXT);
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, data TEXT);
				",
				schema, schema, table1, table2
			))
			.await
			.expect("Failed to create test schema and tables");

		// Query for all tables in all schemas using *.*
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
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&".*", &".*"],
			)
			.await
			.expect("Failed to query tables");

		// Should find tables in both public and test_schema
		let results: Vec<(String, String)> = rows
			.iter()
			.map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
			.collect();

		assert!(
			results.iter().any(|(s, t)| s == "public" && t == &table2),
			"Expected to find {} in public schema",
			table2
		);
		assert!(
			results.iter().any(|(s, t)| s == &schema && t == table1),
			"Expected to find {} in {} schema",
			table1,
			schema
		);

		// Should have multiple schemas represented
		let schemas: std::collections::HashSet<String> =
			results.iter().map(|(s, _)| s.clone()).collect();
		assert!(
			schemas.len() >= 2,
			"Expected tables from at least 2 schemas, got {:?}",
			schemas
		);

		// Cleanup
		client
			.batch_execute(&format!(
				"DROP SCHEMA {} CASCADE; DROP TABLE IF EXISTS public.{};",
				schema, table2
			))
			.await
			.ok();
	}

	#[tokio::test]
	async fn test_list_includes_pg_catalog_tables() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		// Query for tables in pg_catalog schema
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
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&"^pg_catalog$", &".*"],
			)
			.await
			.expect("Failed to query pg_catalog tables");

		// Should find common pg_catalog tables like pg_class, pg_namespace, etc.
		let table_names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(1)).collect();

		assert!(
			!table_names.is_empty(),
			"Expected to find tables in pg_catalog schema"
		);
		assert!(
			table_names.contains(&"pg_class".to_string()),
			"Expected to find pg_class in pg_catalog"
		);
		assert!(
			table_names.contains(&"pg_namespace".to_string()),
			"Expected to find pg_namespace in pg_catalog"
		);
	}

	#[tokio::test]
	async fn test_list_information_schema_when_explicit() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		// Query for tables in information_schema when explicitly specified
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
				ORDER BY 1, 2
				"#,
				&[&"^information_schema$", &".*"],
			)
			.await
			.expect("Failed to query information_schema tables");

		// Should find tables in information_schema
		let table_names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(1)).collect();

		assert!(
			!table_names.is_empty(),
			"Expected to find tables in information_schema when explicitly queried"
		);
	}

	#[tokio::test]
	async fn test_list_pg_toast_when_explicit() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		// Query for tables in pg_toast when explicitly specified
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
				ORDER BY 1, 2
				"#,
				&[&"^pg_toast$", &".*"],
			)
			.await
			.expect("Failed to query pg_toast tables");

		// pg_toast might be empty or have toast tables depending on database state
		// The important thing is the query succeeds without excluding pg_toast
		let schemas: Vec<String> = rows.iter().map(|row| row.get::<_, String>(0)).collect();
		for schema in &schemas {
			assert_eq!(schema, "pg_toast", "All results should be from pg_toast");
		}
	}

	#[tokio::test]
	async fn test_list_excludes_information_schema_by_default() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		// Query with *.* pattern (all schemas) should exclude information_schema
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
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&".*", &".*"],
			)
			.await
			.expect("Failed to query tables");

		let schemas: Vec<String> = rows.iter().map(|row| row.get::<_, String>(0)).collect();
		assert!(
			!schemas.iter().any(|s| s == "information_schema"),
			"information_schema should be excluded by default"
		);
		assert!(
			!schemas.iter().any(|s| s == "pg_toast"),
			"pg_toast should be excluded by default"
		);
	}

	#[tokio::test]
	async fn test_list_indexes_in_public_schema() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_micros();
		let table = format!("psql2_test_idx_table_{}", timestamp);
		let index1 = format!("psql2_test_idx1_{}", timestamp);
		let index2 = format!("psql2_test_idx2_{}", timestamp);

		client
			.batch_execute(&format!(
				"
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, name TEXT, email TEXT);
				CREATE INDEX {} ON public.{} (name);
				CREATE INDEX {} ON public.{} (email);
				",
				table, index1, table, index2, table
			))
			.await
			.expect("Failed to create test table and indexes");

		// Query for indexes in public schema
		let rows = client
			.query(
				r#"
				SELECT
					n.nspname AS "Schema",
					c.relname AS "Name",
					t.relname AS "Table",
					pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				LEFT JOIN pg_catalog.pg_index i ON c.oid = i.indexrelid
				LEFT JOIN pg_catalog.pg_class t ON i.indrelid = t.oid
				WHERE c.relkind = 'i'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&"^public$", &".*"],
			)
			.await
			.expect("Failed to query indexes");

		// Check that we have at least our test indexes
		let index_names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(1)).collect();
		assert!(
			index_names.contains(&index1),
			"Expected to find {} in results",
			index1
		);
		assert!(
			index_names.contains(&index2),
			"Expected to find {} in results",
			index2
		);

		// Cleanup
		client
			.batch_execute(&format!("DROP TABLE IF EXISTS public.{} CASCADE;", table))
			.await
			.ok();
	}

	#[tokio::test]
	async fn test_list_indexes_with_detail() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_micros();
		let table = format!("psql2_test_idx_detail_{}", timestamp);
		let index = format!("psql2_test_idx_detail_idx_{}", timestamp);

		client
			.batch_execute(&format!(
				"
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, name TEXT);
				CREATE INDEX {} ON public.{} (name);
				",
				table, index, table
			))
			.await
			.expect("Failed to create test table and index");

		// Query with detail (additional columns)
		let pattern = format!("^{}$", index);
		let rows = client
			.query(
				r#"
				SELECT
					n.nspname AS "Schema",
					c.relname AS "Name",
					t.relname AS "Table",
					pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size",
					pg_catalog.pg_get_userbyid(c.relowner) AS "Owner",
					CASE c.relpersistence
						WHEN 'p' THEN 'permanent'
						WHEN 'u' THEN 'unlogged'
						WHEN 't' THEN 'temporary'
					END AS "Persistence",
					am.amname AS "Access method",
					CASE
						WHEN c.relacl IS NULL THEN NULL
						ELSE pg_catalog.array_to_string(c.relacl, E'\n')
					END AS "ACL"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				LEFT JOIN pg_catalog.pg_index i ON c.oid = i.indexrelid
				LEFT JOIN pg_catalog.pg_class t ON i.indrelid = t.oid
				LEFT JOIN pg_catalog.pg_am am ON c.relam = am.oid
				WHERE c.relkind = 'i'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&"^public$", &pattern],
			)
			.await
			.expect("Failed to query indexes with detail");

		assert_eq!(rows.len(), 1);
		let row = &rows[0];

		// Check all columns are present
		let schema: String = row.get(0);
		let name: String = row.get(1);
		let table_name: String = row.get(2);
		let size: String = row.get(3);
		let owner: String = row.get(4);
		let persistence: String = row.get(5);
		let access_method: Option<String> = row.get(6);
		let _acl: Option<String> = row.get(7);

		assert_eq!(schema, "public");
		assert_eq!(name, index);
		assert_eq!(table_name, table);
		assert!(!size.is_empty());
		assert!(!owner.is_empty());
		assert_eq!(persistence, "permanent");
		assert!(access_method.is_some(), "Access method should be present");

		// Cleanup
		client
			.batch_execute(&format!("DROP TABLE IF EXISTS public.{} CASCADE;", table))
			.await
			.ok();
	}

	#[tokio::test]
	async fn test_list_indexes_with_pattern_match() {
		let pool = get_test_pool().await;
		let client = pool.get().await.expect("Failed to get client");

		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_micros();
		let table = format!("psql2_test_idx_pat_{}", timestamp);
		let index1 = format!("psql2_idx_match_{}", timestamp);
		let index2 = format!("psql2_idx_nomatch_{}", timestamp);

		client
			.batch_execute(&format!(
				"
				CREATE TABLE public.{} (id SERIAL PRIMARY KEY, name TEXT, email TEXT);
				CREATE INDEX {} ON public.{} (name);
				CREATE INDEX {} ON public.{} (email);
				",
				table, index1, table, index2, table
			))
			.await
			.expect("Failed to create test table and indexes");

		// Query for indexes matching "psql2_idx_match*" pattern
		let pattern = format!("^psql2_idx_match_{}$", timestamp);
		let rows = client
			.query(
				r#"
				SELECT
					n.nspname AS "Schema",
					c.relname AS "Name",
					t.relname AS "Table",
					pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size"
				FROM pg_catalog.pg_class c
				LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
				LEFT JOIN pg_catalog.pg_index i ON c.oid = i.indexrelid
				LEFT JOIN pg_catalog.pg_class t ON i.indrelid = t.oid
				WHERE c.relkind = 'i'
					AND n.nspname ~ $1
					AND c.relname ~ $2
					AND n.nspname NOT IN ('information_schema', 'pg_toast')
				ORDER BY 1, 2
				"#,
				&[&"^public$", &pattern],
			)
			.await
			.expect("Failed to query indexes");

		// Should only match index1
		let index_names: Vec<String> = rows.iter().map(|row| row.get::<_, String>(1)).collect();
		assert_eq!(index_names.len(), 1);
		assert!(index_names.contains(&index1));
		assert!(!index_names.contains(&index2));

		// Cleanup
		client
			.batch_execute(&format!("DROP TABLE IF EXISTS public.{} CASCADE;", table))
			.await
			.ok();
	}
}
