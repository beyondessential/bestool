use bestool_psql2::{highlighter::Theme, PsqlConfig};

#[test]
fn test_config_with_all_fields() {
	let config = PsqlConfig {
		connection_string: "postgresql://user:pass@localhost:5432/testdb".to_string(),
		user: Some("admin".to_string()),
		theme: Theme::Dark,
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
		},
		PsqlConfig {
			connection_string: "postgresql://localhost/db".to_string(),
			user: None,
			theme: Theme::Dark,
		},
		PsqlConfig {
			connection_string: "postgresql://localhost/db".to_string(),
			user: None,
			theme: Theme::Auto,
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
	};

	let config2 = config1.clone();

	assert_eq!(config1.connection_string, config2.connection_string);
	assert_eq!(config1.user, config2.user);
}
