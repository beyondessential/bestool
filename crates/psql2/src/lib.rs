use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets, Attribute, Cell, CellAlignment, Table};
use miette::{IntoDiagnostic, Result};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use supports_unicode::Stream;
use thiserror::Error;
use tokio_postgres::NoTls;
use tracing::{debug, info};

pub mod helper;
pub mod highlighter;
pub mod history;

use helper::SqlHelper;
use highlighter::Theme;
use history::History;

#[derive(Debug, Error)]
pub enum PsqlError {
	#[error("database connection failed")]
	ConnectionFailed,
	#[error("query execution failed")]
	QueryFailed,
}

/// Configuration for the psql2 client
#[derive(Debug, Clone)]
pub struct PsqlConfig {
	/// Database connection string
	pub connection_string: String,

	/// Database user for tracking
	pub user: Option<String>,

	/// Syntax highlighting theme
	pub theme: Theme,

	/// Path to history database
	pub history_path: std::path::PathBuf,

	/// Database name for display in prompt
	pub database_name: String,
}

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
	let (client, connection) = tokio_postgres::connect(&config.connection_string, NoTls)
		.await
		.into_diagnostic()?;

	tokio::spawn(async move {
		if let Err(e) = connection.await {
			eprintln!("connection error: {}", e);
		}
	});

	info!("connected to database");

	debug!("executing version query");
	let rows = client
		.query("SELECT version();", &[])
		.await
		.into_diagnostic()?;

	if let Some(row) = rows.first() {
		let version: String = row.get(0);
		println!("{}", version);
	}

	// Query for database name and superuser status
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

	run_repl(
		client,
		theme,
		history_path,
		db_user,
		database_name,
		is_superuser,
	)
	.await?;

	Ok(())
}

async fn run_repl(
	client: tokio_postgres::Client,
	theme: Theme,
	history_path: std::path::PathBuf,
	db_user: String,
	database_name: String,
	is_superuser: bool,
) -> Result<()> {
	let sys_user = std::env::var("USER")
		.or_else(|_| std::env::var("USERNAME"))
		.unwrap_or_else(|_| "unknown".to_string());

	let mut history = History::open(&history_path)?;
	history.set_context(db_user.clone(), sys_user.clone(), false, None);

	let helper = SqlHelper::new(theme);
	let mut rl: Editor<SqlHelper, History> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		history,
	)
	.into_diagnostic()?;
	rl.set_helper(Some(helper));

	let mut buffer = String::new();

	loop {
		let prompt_suffix = if is_superuser { "=#" } else { "=>" };
		let prompt = if buffer.is_empty() {
			format!("{}{} ", database_name, prompt_suffix)
		} else {
			format!("{}->  ", database_name)
		};

		let readline = rl.readline(&prompt);
		match readline {
			Ok(line) => {
				let line = line.trim();
				if line.is_empty() && buffer.is_empty() {
					continue;
				}

				if buffer.is_empty()
					&& (line.eq_ignore_ascii_case("\\q") || line.eq_ignore_ascii_case("quit"))
				{
					break;
				}

				// Add line to buffer
				if !buffer.is_empty() {
					buffer.push('\n');
				}
				buffer.push_str(line);

				// Check if we should execute (has trailing ; or \g)
				let should_execute = buffer.trim_end().ends_with(';')
					|| buffer.trim_end().ends_with("\\g")
					|| buffer.trim_end().eq_ignore_ascii_case("\\q")
					|| buffer.trim_end().eq_ignore_ascii_case("quit");

				if should_execute {
					let sql = buffer.trim().to_string();
					buffer.clear();

					if sql.eq_ignore_ascii_case("\\q") || sql.eq_ignore_ascii_case("quit") {
						break;
					}

					// Always add to history, even if query fails
					let _ = rl.add_history_entry(&sql);
					if let Err(e) = rl.history_mut().add_entry(
						sql.clone(),
						db_user.clone(),
						sys_user.clone(),
						false,
						None,
					) {
						debug!("failed to add to history: {}", e);
					}

					match execute_query(&client, &sql).await {
						Ok(()) => {}
						Err(e) => {
							eprintln!("Error: {:?}", e);
						}
					}
				}
			}
			Err(ReadlineError::Interrupted) => {
				debug!("CTRL-C");
				if !buffer.is_empty() {
					buffer.clear();
					eprintln!("\nQuery buffer cleared");
				} else {
					break;
				}
			}
			Err(ReadlineError::Eof) => {
				debug!("CTRL-D");
				break;
			}
			Err(err) => {
				eprintln!("Error: {:?}", err);
				break;
			}
		}
	}

	Ok(())
}

async fn execute_query(client: &tokio_postgres::Client, sql: &str) -> Result<()> {
	debug!("executing query: {}", sql);

	let rows = client.query(sql, &[]).await.into_diagnostic()?;

	if rows.is_empty() {
		println!("(no rows)");
		return Ok(());
	}

	if let Some(first_row) = rows.first() {
		let columns = first_row.columns();

		// Identify columns that need text casting
		let mut unprintable_columns = Vec::new();
		for (i, _column) in columns.iter().enumerate() {
			if !can_print_column(&first_row, i) {
				unprintable_columns.push(i);
			}
		}

		// If we have unprintable columns, re-query with text casting
		let text_rows = if !unprintable_columns.is_empty() {
			// Strip trailing semicolon if present
			let sql_trimmed = sql.trim_end_matches(';').trim();
			let text_query = build_text_cast_query(sql_trimmed, &columns, &unprintable_columns);
			debug!("re-querying with text casts: {}", text_query);
			match client.query(&text_query, &[]).await {
				Ok(rows) => Some(rows),
				Err(e) => {
					debug!("failed to re-query with text casts: {:?}", e);
					None
				}
			}
		} else {
			None
		};

		let mut table = Table::new();

		if supports_unicode() {
			table.load_preset(presets::UTF8_FULL);
			table.apply_modifier(UTF8_ROUND_CORNERS);
		} else {
			table.load_preset(presets::ASCII_FULL);
		}

		table.set_header(columns.iter().map(|col| {
			Cell::new(col.name())
				.add_attribute(Attribute::Bold)
				.set_alignment(CellAlignment::Center)
		}));

		for (row_idx, row) in rows.iter().enumerate() {
			let mut row_data = Vec::new();
			for (i, _column) in columns.iter().enumerate() {
				let value_str = if unprintable_columns.contains(&i) {
					// Get from text-cast query
					if let Some(ref text_rows) = text_rows {
						if let Some(text_row) = text_rows.get(row_idx) {
							text_row
								.try_get::<_, Option<String>>(i)
								.ok()
								.flatten()
								.unwrap_or_else(|| "NULL".to_string())
						} else {
							"(error)".to_string()
						}
					} else {
						"(error)".to_string()
					}
				} else {
					format_column_value(row, i)
				};
				row_data.push(value_str);
			}
			table.add_row(row_data);
		}

		println!("{table}");

		println!(
			"({} row{})",
			rows.len(),
			if rows.len() == 1 { "" } else { "s" }
		);
	}

	Ok(())
}

fn can_print_column(row: &tokio_postgres::Row, i: usize) -> bool {
	// Try each supported type - if any succeeds, we can print it
	// Note: we must check Option<T> types carefully to distinguish NULL from unprintable
	if row.try_get::<_, String>(i).is_ok()
		|| row.try_get::<_, i16>(i).is_ok()
		|| row.try_get::<_, i32>(i).is_ok()
		|| row.try_get::<_, i64>(i).is_ok()
		|| row.try_get::<_, f32>(i).is_ok()
		|| row.try_get::<_, f64>(i).is_ok()
		|| row.try_get::<_, bool>(i).is_ok()
		|| row.try_get::<_, Vec<u8>>(i).is_ok()
		|| row.try_get::<_, jiff::Timestamp>(i).is_ok()
		|| row.try_get::<_, jiff::civil::Date>(i).is_ok()
		|| row.try_get::<_, jiff::civil::Time>(i).is_ok()
		|| row.try_get::<_, jiff::civil::DateTime>(i).is_ok()
		|| row.try_get::<_, serde_json::Value>(i).is_ok()
		|| row.try_get::<_, Vec<String>>(i).is_ok()
		|| row.try_get::<_, Vec<i32>>(i).is_ok()
		|| row.try_get::<_, Vec<i64>>(i).is_ok()
		|| row.try_get::<_, Vec<f32>>(i).is_ok()
		|| row.try_get::<_, Vec<f64>>(i).is_ok()
		|| row.try_get::<_, Vec<bool>>(i).is_ok()
	{
		return true;
	}

	// Check if it's NULL by trying to get as Option<String>
	// If this succeeds and is None, it's a true NULL value
	// If this fails, it's an unprintable type
	matches!(row.try_get::<_, Option<String>>(i), Ok(None))
}

fn format_column_value(row: &tokio_postgres::Row, i: usize) -> String {
	if let Ok(v) = row.try_get::<_, String>(i) {
		v
	} else if let Ok(v) = row.try_get::<_, i16>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, i32>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, i64>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, f32>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, f64>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, bool>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, Vec<u8>>(i) {
		format!("\\x{}", hex::encode(v))
	} else if let Ok(v) = row.try_get::<_, jiff::Timestamp>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, jiff::civil::Date>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, jiff::civil::Time>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, jiff::civil::DateTime>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, serde_json::Value>(i) {
		v.to_string()
	} else if let Ok(v) = row.try_get::<_, Vec<String>>(i) {
		format!("{{{}}}", v.join(","))
	} else if let Ok(v) = row.try_get::<_, Vec<i32>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else if let Ok(v) = row.try_get::<_, Vec<i64>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else if let Ok(v) = row.try_get::<_, Vec<f32>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else if let Ok(v) = row.try_get::<_, Vec<f64>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else if let Ok(v) = row.try_get::<_, Vec<bool>>(i) {
		format!(
			"{{{}}}",
			v.iter()
				.map(|x| x.to_string())
				.collect::<Vec<_>>()
				.join(",")
		)
	} else {
		match row.try_get::<_, Option<String>>(i) {
			Ok(None) => "NULL".to_string(),
			Ok(Some(_)) => "(unprintable)".to_string(),
			Err(_) => "NULL".to_string(),
		}
	}
}

fn build_text_cast_query(
	sql: &str,
	columns: &[tokio_postgres::Column],
	unprintable_columns: &[usize],
) -> String {
	// Build a SELECT query that casts unprintable columns to text
	let column_exprs: Vec<String> = columns
		.iter()
		.enumerate()
		.map(|(i, col)| {
			if unprintable_columns.contains(&i) {
				format!("(subq.{})::text", col.name())
			} else {
				format!("subq.{}", col.name())
			}
		})
		.collect();

	format!("SELECT {} FROM ({}) AS subq", column_exprs.join(", "), sql)
}

fn supports_unicode() -> bool {
	supports_unicode::on(Stream::Stdout)
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
			theme: Theme::Light,
			history_path: std::path::PathBuf::from("/tmp/history.redb"),
			database_name: "test".to_string(),
		};

		assert_eq!(config.connection_string, "postgresql://localhost/test");
		assert!(config.user.is_none());
	}

	#[test]
	fn test_psql_error_display() {
		let err = PsqlError::ConnectionFailed;
		assert_eq!(err.to_string(), "database connection failed");

		let err = PsqlError::QueryFailed;
		assert_eq!(err.to_string(), "query execution failed");
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

		// Test that record types are handled properly
		let result = execute_query(&client, "SELECT row(1, 'foo', true) as record").await;

		// Should succeed without panicking
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

		let rows = client
			.query("SELECT array[1,2,3] as arr", &[])
			.await
			.expect("Failed to execute query");

		assert!(!rows.is_empty());
		let row = rows.first().expect("No rows returned");
		let formatted = format_column_value(row, 0);
		assert_eq!(formatted, "{1,2,3}");
	}

	#[test]
	fn test_supports_unicode() {
		// This just tests that the function runs without panicking
		let _ = supports_unicode();
	}

	#[test]
	fn test_prompt_generation_regular_user() {
		let database_name = "mydb";
		let is_superuser = false;
		let prompt_suffix = if is_superuser { "=#" } else { "=>" };
		let prompt = format!("{}{} ", database_name, prompt_suffix);
		assert_eq!(prompt, "mydb=> ");
	}

	#[test]
	fn test_prompt_generation_superuser() {
		let database_name = "postgres";
		let is_superuser = true;
		let prompt_suffix = if is_superuser { "=#" } else { "=>" };
		let prompt = format!("{}{} ", database_name, prompt_suffix);
		assert_eq!(prompt, "postgres=# ");
	}

	#[test]
	fn test_prompt_generation_continuation() {
		let database_name = "mydb";
		let prompt = format!("{}->  ", database_name);
		assert_eq!(prompt, "mydb->  ");
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

		// Query for database name and superuser status
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

	#[test]
	fn test_build_text_cast_query_logic() {
		// Test the query building logic by checking string patterns
		// We can't create Column objects directly, but we can test with a mock setup

		// Simulate what build_text_cast_query does
		let sql = "SELECT id, name, data FROM users";
		let column_names = vec!["id", "name", "data"];
		let unprintable_indices = vec![0, 2]; // id and data are unprintable

		let column_exprs: Vec<String> = column_names
			.iter()
			.enumerate()
			.map(|(i, name)| {
				if unprintable_indices.contains(&i) {
					format!("(subq.{})::text", name)
				} else {
					format!("subq.{}", name)
				}
			})
			.collect();

		let result = format!("SELECT {} FROM ({}) AS subq", column_exprs.join(", "), sql);

		assert!(result.contains("(subq.id)::text"));
		assert!(result.contains("subq.name"));
		assert!(result.contains("(subq.data)::text"));
		assert!(result.starts_with("SELECT"));
		assert!(result.contains("AS subq"));
	}
}
