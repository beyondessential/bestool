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
}

/// Run the psql2 client
pub async fn run(config: PsqlConfig) -> Result<()> {
	let theme = config.theme;
	let history_path = config.history_path.clone();
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

	run_repl(client, theme, history_path, db_user).await?;

	Ok(())
}

async fn run_repl(
	client: tokio_postgres::Client,
	theme: Theme,
	history_path: std::path::PathBuf,
	db_user: String,
) -> Result<()> {
	let sys_user = std::env::var("USER")
		.or_else(|_| std::env::var("USERNAME"))
		.unwrap_or_else(|_| "unknown".to_string());

	let mut history = History::open(&history_path)?;
	history.set_context(db_user.clone(), sys_user.clone(), false, None);

	let helper = SqlHelper::new(theme);
	let mut rl: Editor<SqlHelper, History> = Editor::with_history(
		rustyline::Config::builder().auto_add_history(false).build(),
		history,
	)
	.into_diagnostic()?;
	rl.set_helper(Some(helper));

	loop {
		let readline = rl.readline("psql2> ");
		match readline {
			Ok(line) => {
				let line = line.trim();
				if line.is_empty() {
					continue;
				}

				if line.eq_ignore_ascii_case("\\q") || line.eq_ignore_ascii_case("quit") {
					break;
				}

				// Always add to history, even if query fails
				let _ = rl.add_history_entry(line);
				if let Err(e) = rl.history_mut().add_entry(
					line.to_string(),
					db_user.clone(),
					sys_user.clone(),
					false,
					None,
				) {
					debug!("failed to add to history: {}", e);
				}

				match execute_query(&client, line).await {
					Ok(()) => {}
					Err(e) => {
						eprintln!("Error: {:?}", e);
					}
				}
			}
			Err(ReadlineError::Interrupted) => {
				debug!("CTRL-C");
				break;
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
			let text_query = build_text_cast_query(sql, &columns, &unprintable_columns);
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
	row.try_get::<_, String>(i).is_ok()
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
		|| row.try_get::<_, Option<String>>(i).ok().flatten().is_none()
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
		};

		assert_eq!(config.connection_string, "postgresql://localhost/test");
		assert_eq!(config.user, Some("testuser".to_string()));
	}

	#[test]
	fn test_psql_config_no_user() {
		let config = PsqlConfig {
			connection_string: "postgresql://localhost/test".to_string(),
			user: None,
			theme: Theme::Light,
			history_path: std::path::PathBuf::from("/tmp/history.redb"),
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
}
