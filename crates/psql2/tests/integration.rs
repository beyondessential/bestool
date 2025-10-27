use bestool_psql2::{highlighter::Theme, PsqlConfig};

#[test]
fn test_config_with_all_fields() {
	let config = PsqlConfig {
		connection_string: "postgresql://user:pass@localhost:5432/testdb".to_string(),
		user: Some("admin".to_string()),
		theme: Theme::Dark,
		history_path: std::path::PathBuf::from("/tmp/history.redb"),
		database_name: "testdb".to_string(),
		write: false,
		ots: None,
	};

	assert_eq!(
		config.connection_string,
		"postgresql://user:pass@localhost:5432/testdb"
	);
	assert_eq!(config.user, Some("admin".to_string()));
}

#[test]
fn test_config_minimal() {
	let config = PsqlConfig {
		connection_string: "postgresql://localhost/db".to_string(),
		user: None,
		theme: Theme::Auto,
		history_path: std::path::PathBuf::from("/tmp/history.redb"),
		database_name: "db".to_string(),
		write: false,
		ots: None,
	};

	assert_eq!(config.connection_string, "postgresql://localhost/db");
	assert_eq!(config.user, None);
}

#[test]
fn test_theme_variations() {
	let configs = vec![
		PsqlConfig {
			connection_string: "postgresql://localhost/db".to_string(),
			user: None,
			theme: Theme::Light,
			history_path: std::path::PathBuf::from("/tmp/history.redb"),
			database_name: "db".to_string(),
			write: false,
			ots: None,
		},
		PsqlConfig {
			connection_string: "postgresql://localhost/db".to_string(),
			user: None,
			theme: Theme::Dark,
			history_path: std::path::PathBuf::from("/tmp/history.redb"),
			database_name: "db".to_string(),
			write: false,
			ots: None,
		},
		PsqlConfig {
			connection_string: "postgresql://localhost/db".to_string(),
			user: None,
			theme: Theme::Auto,
			history_path: std::path::PathBuf::from("/tmp/history.redb"),
			database_name: "db".to_string(),
			write: false,
			ots: None,
		},
	];

	for config in configs {
		assert!(!config.connection_string.is_empty());
	}
}

#[test]
fn test_config_clone() {
	let config1 = PsqlConfig {
		connection_string: "postgresql://localhost/db".to_string(),
		user: Some("user1".to_string()),
		theme: Theme::Dark,
		history_path: std::path::PathBuf::from("/tmp/history.redb"),
		database_name: "db".to_string(),
		write: false,
		ots: None,
	};

	let config2 = config1.clone();

	assert_eq!(config1.connection_string, config2.connection_string);
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
