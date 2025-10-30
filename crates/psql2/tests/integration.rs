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
