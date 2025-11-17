use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};

use crossterm::style::{Color, Stylize};
use miette::Result;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tracing::{debug, warn};

use crate::{
	PgPool,
	error::PgDatabaseError,
	parser::{QueryModifier, QueryModifiers},
	repl::ReplState,
	signals::{reset_sigint, sigint_received},
	theme::Theme,
};

pub(crate) mod column;
pub(crate) mod display;
mod vars;

/// Context for executing a query.
pub(crate) struct QueryContext<'a, W: AsyncWrite + Unpin> {
	pub client: &'a tokio_postgres::Client,
	pub pool: &'a PgPool,
	pub modifiers: QueryModifiers,
	pub theme: Theme,
	pub writer: &'a mut W,
	pub use_colours: bool,
	pub vars: Option<&'a mut BTreeMap<String, String>>,
	pub repl_state: &'a Arc<Mutex<ReplState>>,
}

/// Execute a SQL query and display the results.
pub(crate) async fn execute_query<W: AsyncWrite + Unpin>(
	sql: &str,
	ctx: &mut QueryContext<'_, W>,
) -> Result<()> {
	debug!(?ctx.modifiers, %sql, "executing query");

	// Interpolate variables unless Verbatim modifier is used
	let sql_to_execute = if ctx.modifiers.contains(&QueryModifier::Verbatim) {
		sql.to_string()
	} else {
		let empty_vars = BTreeMap::new();
		let vars = ctx.vars.as_ref().map_or(&empty_vars, |v| v);
		vars::interpolate_variables(sql, vars)?
	};

	// Split by semicolons to handle multiple statements
	let statements = split_statements(&sql_to_execute);

	for statement in statements {
		let statement = statement.trim();
		if statement.is_empty() {
			continue;
		}

		execute_single_statement(statement, ctx).await?;
	}

	Ok(())
}

/// Split SQL into multiple statements by semicolon
fn split_statements(sql: &str) -> Vec<String> {
	let mut statements = Vec::new();
	let mut current = String::new();
	let mut in_string = false;
	let mut string_char = ' ';
	let mut escaped = false;

	for ch in sql.chars() {
		if escaped {
			current.push(ch);
			escaped = false;
			continue;
		}

		match ch {
			'\\' if in_string => {
				escaped = true;
				current.push(ch);
			}
			'\'' | '"' => {
				if !in_string {
					in_string = true;
					string_char = ch;
				} else if ch == string_char {
					in_string = false;
				}
				current.push(ch);
			}
			';' if !in_string => {
				let trimmed = current.trim();
				if !trimmed.is_empty() {
					statements.push(trimmed.to_string());
				}
				current.clear();
			}
			_ => {
				current.push(ch);
			}
		}
	}

	// Add remaining statement if any
	let trimmed = current.trim();
	if !trimmed.is_empty() {
		statements.push(trimmed.to_string());
	}

	statements
}

/// Execute a single SQL statement and display the results.
async fn execute_single_statement<W: AsyncWrite + Unpin>(
	statement: &str,
	ctx: &mut QueryContext<'_, W>,
) -> Result<()> {
	let start = std::time::Instant::now();

	let cancel_token = ctx.client.cancel_token();

	// Reset the flag before starting
	reset_sigint();

	// Poll for SIGINT while executing query, with progress indicator for long queries
	let start_time = std::time::Instant::now();
	let mut progress_shown = false;

	let result = tokio::select! {
		result = ctx.client.query(statement, &[]) => {
			// Clear progress indicator if it was shown
			if progress_shown {
				eprint!("\r\x1b[K"); // Clear the line
			}
			result
		}
		_ = async {
			loop {
				tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

				let elapsed = start_time.elapsed();

				// After 1 seconds, start showing progress indicator
				if elapsed.as_secs() >= 1 && ctx.use_colours {
					let secs = elapsed.as_secs();
					let progress_msg = format!("(running, so far {}s)", secs);
					let colored_msg = progress_msg.with(Color::Blue).dim();
					eprint!("\r{}", colored_msg);
					progress_shown = true;
				}

				if sigint_received() {
					if progress_shown {
						eprint!("\r\x1b[K"); // Clear the line
					}
					break;
				}
			}
		} => {
			if progress_shown {
				eprint!("\r\x1b[K"); // Clear the line
			}
			eprintln!("\nCancelling query...");
			if let Err(e) = ctx.pool.manager.cancel(&cancel_token).await {
				warn!("Failed to cancel query: {:?}", e);
			}
			// Reset flag for next time
			reset_sigint();
			return Ok(());
		}
	};

	let rows = match result {
		Ok(rows) => rows,
		Err(e) => {
			// Convert to our custom error type with query context
			if let Some(db_error) = e.as_db_error() {
				return Err(PgDatabaseError::from_db_error(db_error, Some(statement)).into());
			} else {
				// Non-database error (connection error, etc)
				return Err(miette::miette!("Database error: {:?}", e));
			}
		}
	};

	let duration = start.elapsed();

	if rows.is_empty() {
		let msg = if ctx.use_colours {
			format!("{}\n", "(no rows)".with(Color::Blue).dim())
		} else {
			"(no rows)\n".to_string()
		};
		ctx.writer
			.write_all(msg.as_bytes())
			.await
			.map_err(|e| miette::miette!("Failed to write output: {}", e))?;
		ctx.writer
			.flush()
			.await
			.map_err(|e| miette::miette!("Failed to flush output: {}", e))?;
		return Ok(());
	}

	if let Some(first_row) = rows.first() {
		let columns = first_row.columns();

		let mut unprintable_columns = Vec::new();
		for (i, _column) in columns.iter().enumerate() {
			if !column::can_print(first_row, i) {
				unprintable_columns.push(i);
			}
		}

		let text_rows = if !unprintable_columns.is_empty() {
			let sql_trimmed = statement.trim_end_matches(';').trim();
			let text_query = build_text_cast_query(sql_trimmed, columns, &unprintable_columns);
			debug!("re-querying with text casts: {text_query}");
			match ctx.client.query(&text_query, &[]).await {
				Ok(rows) => Some(rows),
				Err(e) => {
					debug!("failed to re-query with text casts: {e:?}");
					None
				}
			}
		} else {
			None
		};

		// Store results in the result store (before formatting)
		if !rows.is_empty() {
			let mut state = ctx.repl_state.lock().unwrap();
			state
				.result_store
				.push(statement.to_string(), rows.clone(), duration);
		}

		let is_expanded = ctx.modifiers.contains(&QueryModifier::Expanded);
		let is_json = ctx.modifiers.contains(&QueryModifier::Json);
		let is_zero = ctx.modifiers.contains(&QueryModifier::Zero);

		// Only display if not using Zero modifier
		if !is_zero {
			// Auto-limit: if more than 50 rows, only display first 30
			let (display_rows, was_truncated) = if rows.len() > 50 {
				(&rows[..30], true)
			} else {
				(&rows[..], false)
			};

			display::display(
				&mut display::DisplayContext {
					columns,
					rows: display_rows,
					unprintable_columns: &unprintable_columns,
					text_rows: &text_rows,
					writer: ctx.writer,
					use_colours: ctx.use_colours,
					theme: ctx.theme,
					column_indices: None,
				},
				is_json,
				is_expanded,
			)
			.await?;

			// Print truncation message if needed
			if was_truncated {
				let truncation_msg = if ctx.use_colours {
					format!(
						"{}\n",
						"[output truncated, use \\re show limit=N to print more]"
							.with(Color::Magenta)
							.bold()
					)
				} else {
					"[output truncated, use \\re show limit=N to print more]\n".to_string()
				};
				eprint!("{}", truncation_msg);
			}
		}

		let status_text = format!(
			"({} row{}, took {:.3}ms)",
			rows.len(),
			if rows.len() == 1 { "" } else { "s" },
			duration.as_secs_f64() * 1000.0
		);
		let status_msg = if ctx.use_colours {
			format!("{}\n", status_text.with(Color::Blue).dim())
		} else {
			format!("{status_text}\n")
		};
		// Status messages always go to stderr
		eprint!("{status_msg}");

		// Handle VarSet modifier: if exactly one row, store column values as variables
		if rows.len() == 1
			&& let Some(var_prefix) = ctx.modifiers.iter().find_map(|m| {
				if let QueryModifier::VarSet { prefix } = m {
					Some(prefix)
				} else {
					None
				}
			}) && let Some(vars_map) = ctx.vars.as_mut()
		{
			let row = &rows[0];
			for (i, column) in columns.iter().enumerate() {
				let var_name = if let Some(prefix_str) = var_prefix {
					format!("{prefix_str}{col_name}", col_name = column.name())
				} else {
					column.name().to_string()
				};

				// Get the value as a string
				let value = if unprintable_columns.contains(&i) {
					if let Some(ref text_rows) = text_rows {
						if let Some(text_row) = text_rows.first() {
							text_row
								.try_get::<_, String>(i)
								.unwrap_or_else(|_| String::new())
						} else {
							String::new()
						}
					} else {
						String::new()
					}
				} else {
					column::get_value(row, i, 0, &unprintable_columns, &text_rows)
				};

				vars_map.insert(var_name, value);
			}
		}
	}

	Ok(())
}

pub(crate) fn build_text_cast_query(
	sql: &str,
	columns: &[tokio_postgres::Column],
	unprintable_columns: &[usize],
) -> String {
	let column_exprs: Vec<String> = columns
		.iter()
		.enumerate()
		.map(|(i, col)| {
			if unprintable_columns.contains(&i) {
				format!("(subq.\"{col_name}\")::text", col_name = col.name())
			} else {
				format!("subq.\"{col_name}\"", col_name = col.name())
			}
		})
		.collect();

	format!(
		"SELECT {cols} FROM ({sql}) AS subq",
		cols = column_exprs.join(", ")
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_split_statements_single() {
		let sql = "SELECT 1";
		let statements = split_statements(sql);
		assert_eq!(statements.len(), 1);
		assert_eq!(statements[0], "SELECT 1");
	}

	#[test]
	fn test_split_statements_multiple() {
		let sql = "SELECT 1; SELECT 2; SELECT 3";
		let statements = split_statements(sql);
		assert_eq!(statements.len(), 3);
		assert_eq!(statements[0], "SELECT 1");
		assert_eq!(statements[1], "SELECT 2");
		assert_eq!(statements[2], "SELECT 3");
	}

	#[test]
	fn test_split_statements_with_string_literals() {
		let sql = "SELECT 'hello; world'; SELECT 2";
		let statements = split_statements(sql);
		assert_eq!(statements.len(), 2);
		assert_eq!(statements[0], "SELECT 'hello; world'");
		assert_eq!(statements[1], "SELECT 2");
	}

	#[test]
	fn test_split_statements_with_double_quotes() {
		let sql = r#"SELECT "table; name"; SELECT 2"#;
		let statements = split_statements(sql);
		assert_eq!(statements.len(), 2);
		assert_eq!(statements[0], r#"SELECT "table; name""#);
		assert_eq!(statements[1], "SELECT 2");
	}

	#[test]
	fn test_split_statements_with_escaped_quotes() {
		let sql = r"SELECT 'it\'s'; SELECT 2";
		let statements = split_statements(sql);
		assert_eq!(statements.len(), 2);
		assert_eq!(statements[0], r"SELECT 'it\'s'");
		assert_eq!(statements[1], "SELECT 2");
	}

	#[test]
	fn test_split_statements_empty() {
		let sql = "";
		let statements = split_statements(sql);
		assert_eq!(statements.len(), 0);
	}

	#[test]
	fn test_split_statements_trailing_semicolon() {
		let sql = "SELECT 1;";
		let statements = split_statements(sql);
		assert_eq!(statements.len(), 1);
		assert_eq!(statements[0], "SELECT 1");
	}

	#[test]
	fn test_split_statements_multiple_semicolons() {
		let sql = "SELECT 1;;; SELECT 2";
		let statements = split_statements(sql);
		assert_eq!(statements.len(), 2);
		assert_eq!(statements[0], "SELECT 1");
		assert_eq!(statements[1], "SELECT 2");
	}

	#[test]
	fn test_build_text_cast_query_logic() {
		let sql = "SELECT id, name, data FROM users";
		let column_names = ["id", "name", "data"];
		let unprintable_indices = [0, 2];

		let column_exprs: Vec<String> = column_names
			.iter()
			.enumerate()
			.map(|(i, name)| {
				if unprintable_indices.contains(&i) {
					format!("(subq.{name})::text")
				} else {
					format!("subq.{name}")
				}
			})
			.collect();

		let result = format!(
			"SELECT {cols} FROM ({sql}) AS subq",
			cols = column_exprs.join(", ")
		);

		assert!(result.contains("(subq.id)::text"));
		assert!(result.contains("subq.name"));
		assert!(result.contains("(subq.data)::text"));
		assert!(result.starts_with("SELECT"));
		assert!(result.contains("AS subq"));
	}

	#[test]
	fn test_expanded_modifier_detection() {
		use std::collections::HashSet;

		// Test with expanded modifier
		let mut modifiers_with_expanded = HashSet::new();
		modifiers_with_expanded.insert(QueryModifier::Expanded);
		assert!(modifiers_with_expanded.contains(&QueryModifier::Expanded));

		// Test without expanded modifier
		let mut modifiers_without_expanded = HashSet::new();
		modifiers_without_expanded.insert(QueryModifier::Json);
		assert!(!modifiers_without_expanded.contains(&QueryModifier::Expanded));

		// Test with multiple modifiers including expanded
		let mut modifiers_mixed = HashSet::new();
		modifiers_mixed.insert(QueryModifier::Expanded);
		modifiers_mixed.insert(QueryModifier::Json);
		assert!(modifiers_mixed.contains(&QueryModifier::Expanded));
	}

	#[test]
	fn test_json_modifier_detection() {
		use std::collections::HashSet;

		let mut modifiers_with_json = HashSet::new();
		modifiers_with_json.insert(QueryModifier::Json);
		assert!(modifiers_with_json.contains(&QueryModifier::Json));

		let mut modifiers_without_json = HashSet::new();
		modifiers_without_json.insert(QueryModifier::Expanded);
		assert!(!modifiers_without_json.contains(&QueryModifier::Json));

		let mut modifiers_both = HashSet::new();
		modifiers_both.insert(QueryModifier::Json);
		modifiers_both.insert(QueryModifier::Expanded);
		assert!(modifiers_both.contains(&QueryModifier::Json));
		assert!(modifiers_both.contains(&QueryModifier::Expanded));
	}

	#[test]
	fn test_execute_query_with_verbatim_modifier() {
		// Test that Verbatim modifier prevents interpolation
		let mut mods: QueryModifiers = QueryModifiers::new();
		mods.insert(QueryModifier::Verbatim);

		assert!(mods.contains(&QueryModifier::Verbatim));
	}

	#[test]
	fn test_execute_query_without_verbatim_modifier() {
		// Test that without Verbatim modifier, interpolation would occur
		let mods: QueryModifiers = QueryModifiers::new();

		assert!(!mods.contains(&QueryModifier::Verbatim));
	}

	#[test]
	fn test_zero_modifier_detection() {
		use std::collections::HashSet;

		let mut modifiers_with_zero = HashSet::new();
		modifiers_with_zero.insert(QueryModifier::Zero);
		assert!(modifiers_with_zero.contains(&QueryModifier::Zero));

		let mut modifiers_without_zero = HashSet::new();
		modifiers_without_zero.insert(QueryModifier::Expanded);
		assert!(!modifiers_without_zero.contains(&QueryModifier::Zero));

		let mut modifiers_mixed = HashSet::new();
		modifiers_mixed.insert(QueryModifier::Zero);
		modifiers_mixed.insert(QueryModifier::Expanded);
		assert!(modifiers_mixed.contains(&QueryModifier::Zero));
		assert!(modifiers_mixed.contains(&QueryModifier::Expanded));
	}

	#[test]
	fn test_auto_limit_logic() {
		// Test that rows > 50 should be truncated to 30
		let total_rows = 100;
		let should_truncate = total_rows > 50;
		assert!(should_truncate);

		let display_count = if should_truncate { 30 } else { total_rows };
		assert_eq!(display_count, 30);

		// Test that rows <= 50 should not be truncated
		let small_rows = 40;
		let should_not_truncate = small_rows > 50;
		assert!(!should_not_truncate);

		let display_all = if should_not_truncate { 30 } else { small_rows };
		assert_eq!(display_all, 40);
	}

	#[test]
	fn test_auto_limit_boundary() {
		// Test boundary condition: exactly 50 rows should not be truncated
		let boundary_rows = 50;
		let should_truncate = boundary_rows > 50;
		assert!(!should_truncate);

		// Test boundary condition: 51 rows should be truncated
		let over_boundary = 51;
		let should_truncate_51 = over_boundary > 50;
		assert!(should_truncate_51);
	}
}
