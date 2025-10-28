mod completer;
mod config;
mod parser;
mod query;
mod repl;
mod schema_cache;
mod tls;

pub mod highlighter;
pub mod history;
pub mod ots;

pub use config::{PsqlConfig, PsqlError};
pub use highlighter::Theme;

use miette::{IntoDiagnostic, Result};
use std::sync::Arc;
use tracing::debug;

/// Run the psql2 client
pub async fn run(config: PsqlConfig) -> Result<()> {
	let theme = config.theme;
	let history_path = config.history_path.clone();
	let database_name = config.database_name.clone();
	let db_user = config.user.clone().unwrap_or_else(|| {
		std::env::var("USER")
			.or_else(|_| std::env::var("USERNAME"))
			.unwrap_or_else(|_| "unknown".to_string())
	});

	debug!("connecting to database");
	let tls_connector = tls::make_tls_connector()?;
	let (client, connection) = tokio_postgres::connect(&config.connection_string, tls_connector)
		.await
		.into_diagnostic()?;

	tokio::spawn(async move {
		if let Err(e) = connection.await {
			eprintln!("connection error: {}", e);
		}
	});

	debug!("connected to database");

	if config.write {
		debug!("setting session to read-write mode with autocommit off");
		client
			.batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; BEGIN")
			.await
			.into_diagnostic()?;
	} else {
		debug!("setting session to read-only mode");
		client
			.execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY", &[])
			.await
			.into_diagnostic()?;
	}

	debug!("executing version query");
	let rows = client
		.query("SELECT version();", &[])
		.await
		.into_diagnostic()?;

	if let Some(row) = rows.first() {
		let version: String = row.get(0);
		println!("{}", version);
	}

	let info_rows = client
		.query(
			"SELECT current_database(), current_user, usesuper FROM pg_user WHERE usename = current_user",
			&[],
		)
		.await
		.into_diagnostic()?;

	let (database_name, is_superuser) = if let Some(row) = info_rows.first() {
		let db: String = row.get(0);
		let is_super: bool = row.get(2);
		(db, is_super)
	} else {
		(database_name, false)
	};

	repl::run_repl(
		Arc::new(client),
		theme,
		history_path,
		db_user,
		database_name,
		is_superuser,
		config.connection_string,
		config.write,
		config.ots,
	)
	.await?;

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_psql_config_creation() {
		let config = PsqlConfig {
			connection_string: "postgresql://localhost/test".to_string(),
			user: Some("testuser".to_string()),
			theme: Theme::Dark,
			history_path: std::path::PathBuf::from("/tmp/history.redb"),
			database_name: "test".to_string(),
			write: false,
			ots: None,
		};

		assert_eq!(config.connection_string, "postgresql://localhost/test");
		assert_eq!(config.user, Some("testuser".to_string()));
		assert_eq!(config.database_name, "test");
	}

	#[test]
	fn test_psql_config_no_user() {
		let config = PsqlConfig {
			connection_string: "postgresql://localhost/test".to_string(),
			user: None,
			theme: Theme::Dark,
			history_path: std::path::PathBuf::from("/tmp/history.redb"),
			database_name: "test".to_string(),
			write: false,
			ots: None,
		};

		assert_eq!(config.user, None);
	}

	#[test]
	fn test_psql_error_display() {
		let err = PsqlError::ConnectionFailed;
		assert_eq!(format!("{}", err), "database connection failed");

		let err = PsqlError::QueryFailed;
		assert_eq!(format!("{}", err), "query execution failed");
	}

	#[tokio::test]
	async fn test_text_cast_for_record_types() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) =
			tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
				.await
				.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let result = query::execute_query(
			&client,
			"SELECT row(1, 'foo', true) as record",
			parser::QueryModifiers::new(),
		)
		.await;

		assert!(result.is_ok());
	}

	#[tokio::test]
	async fn test_array_formatting() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) =
			tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
				.await
				.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let result = query::execute_query(
			&client,
			"SELECT ARRAY[1, 2, 3] as numbers",
			parser::QueryModifiers::new(),
		)
		.await;

		assert!(result.is_ok());
	}

	#[tokio::test]
	async fn test_database_info_query() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) =
			tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
				.await
				.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

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
}
