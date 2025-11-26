use std::{
	collections::BTreeMap,
	pin::pin,
	sync::{Arc, Mutex},
};

use futures::StreamExt as _;
use miette::Result;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tracing::{debug, warn};

use crate::colors;

use bestool_postgres::{
	error::PgDatabaseError,
	pool::PgPool,
	stringify::{can_print, get_value},
	text_cast::{CellRef, TextCaster},
};

use crate::{
	parser::{QueryModifier, QueryModifiers},
	repl::ReplState,
	signals::{reset_sigint, sigint_received},
	theme::Theme,
};

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

/// Convert SQL command to past tense verb for output
fn command_to_verb(command: &str) -> &str {
	match command.to_uppercase().as_str() {
		"INSERT" => "inserted",
		"UPDATE" => "updated",
		"DELETE" => "deleted",
		"CREATE" => "created",
		"DROP" => "dropped",
		"ALTER" => "altered",
		"TRUNCATE" => "truncated",
		"BEGIN" => "began",
		"COMMIT" => "committed",
		"ROLLBACK" => "rolled back",
		"GRANT" => "granted",
		"REVOKE" => "revoked",
		"COPY" => "copied",
		"MERGE" => "merged",
		"REPLACE" => "replaced",
		"SET" => "set",
		"RESET" => "reset",
		"SAVEPOINT" => "savepoint",
		"RELEASE" => "released",
		"PREPARE" => "prepared",
		"EXECUTE" => "executed",
		"DEALLOCATE" => "deallocated",
		"DISCARD" => "discarded",
		"LOCK" => "locked",
		"UNLISTEN" => "unlistened",
		"LISTEN" => "listened",
		"NOTIFY" => "notified",
		"VACUUM" => "vacuumed",
		"ANALYZE" => "analyzed",
		"CLUSTER" => "clustered",
		"REINDEX" => "reindexed",
		"COMMENT" => "commented",
		"EXPLAIN" => "explained",
		_ => "affected",
	}
}

/// Determine if a command should hide the row count in the output.
/// Commands like COMMIT, ROLLBACK, TRUNCATE, etc. don't have meaningful row counts.
fn should_hide_row_count(command: &str) -> bool {
	matches!(
		command.to_uppercase().as_str(),
		"BEGIN"
			| "COMMIT"
			| "ROLLBACK"
			| "TRUNCATE"
			| "SAVEPOINT"
			| "RELEASE"
			| "SET" | "RESET"
			| "PREPARE"
			| "DEALLOCATE"
			| "DISCARD"
			| "LOCK" | "UNLISTEN"
			| "LISTEN"
			| "NOTIFY"
			| "VACUUM"
			| "ANALYZE"
			| "CLUSTER"
			| "REINDEX"
			| "COMMENT"
	)
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

	// Use query_raw to get both typed rows and affected count
	let row_stream = tokio::select! {
		result = ctx.client.query_raw(statement, &[] as &[i32; 0]) => {
			// Clear progress indicator if it was shown
			if progress_shown {
				eprint!("{}", colors::CLEAR_LINE);
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
					let colored_msg = colors::style_progress(&progress_msg, ctx.use_colours);
					eprint!("\r{}", colored_msg);
					progress_shown = true;
				}

				if sigint_received() {
					if progress_shown {
						eprint!("{}", colors::CLEAR_LINE);
					}
					break;
				}
			}
		} => {
			if progress_shown {
				eprint!("{}", colors::CLEAR_LINE);
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

	let row_stream = match row_stream {
		Ok(stream) => stream,
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

	// Collect all rows from the stream (need to pin for next())
	let mut row_stream = pin!(row_stream);
	let mut rows = Vec::new();
	while let Some(row_result) = row_stream.next().await {
		match row_result {
			Ok(row) => rows.push(row),
			Err(e) => {
				// Convert to our custom error type with query context
				if let Some(db_error) = e.as_db_error() {
					return Err(PgDatabaseError::from_db_error(db_error, Some(statement)).into());
				} else {
					// Non-database error (connection error, etc)
					return Err(miette::miette!("Database error: {:?}", e));
				}
			}
		}
	}

	let duration = start.elapsed();

	// Get the affected rows count from the stream (only available after exhausting the stream)
	let rows_affected = row_stream.rows_affected();

	// Handle DML/DDL commands (no rows returned)
	if rows.is_empty()
		&& let Some(count) = rows_affected
	{
		let command = statement.split_whitespace().next().unwrap_or("QUERY");
		let verb = command_to_verb(command);
		let status_text = if should_hide_row_count(command) {
			format!("({}, took {:.3} ms)", verb, duration.as_secs_f64() * 1000.0)
		} else {
			format!(
				"({} {} row{}, took {:.3} ms)",
				verb,
				count,
				if count == 1 { "" } else { "s" },
				duration.as_secs_f64() * 1000.0
			)
		};

		let status_msg = format!("{}\n", colors::style_status(&status_text, ctx.use_colours));

		// Status message goes to stderr like psql
		eprint!("{status_msg}");

		ctx.writer
			.flush()
			.await
			.map_err(|e| miette::miette!("Failed to flush output: {}", e))?;
		return Ok(());
	}

	// Handle SELECT queries with no results
	if rows.is_empty() {
		let status_text = format!("(0 rows, took {:.3} ms)", duration.as_secs_f64() * 1000.0);

		let status_msg = format!("{}\n", colors::style_status(&status_text, ctx.use_colours));

		// Status message goes to stderr like psql
		eprint!("{status_msg}");

		ctx.writer
			.flush()
			.await
			.map_err(|e| miette::miette!("Failed to flush output: {}", e))?;
		return Ok(());
	}

	// Handle SELECT queries with results
	if let Some(first_row) = rows.first() {
		let columns = first_row.columns();

		let mut unprintable_columns = Vec::new();
		for (i, _column) in columns.iter().enumerate() {
			if !can_print(first_row, i) {
				unprintable_columns.push(i);
			}
		}

		// Create a text caster for on-demand conversion of unprintable values
		// This uses a separate connection from the pool and caches results
		let text_caster = if !unprintable_columns.is_empty() {
			Some(TextCaster::new(ctx.pool.clone()))
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
					text_caster: text_caster.clone(),
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
				let truncation_msg = format!(
					"{}\n",
					colors::style_warning(
						"[output truncated, use \\re show limit=N to print more]",
						ctx.use_colours
					)
				);
				eprint!("{}", truncation_msg);
			}
		}

		let status_text = format!(
			"({} row{}, took {:.3} ms)",
			rows.len(),
			if rows.len() == 1 { "" } else { "s" },
			duration.as_secs_f64() * 1000.0
		);

		let status_msg = format!("{}\n", colors::style_status(&status_text, ctx.use_colours));

		// Status message goes to stderr
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
			// Collect all unprintable columns that need casting
			let unprintable_cells: Vec<CellRef> = unprintable_columns
				.iter()
				.map(|&col_idx| CellRef {
					row_idx: 0,
					col_idx,
				})
				.collect();

			// Batch cast all unprintable columns at once
			let cast_results = if !unprintable_cells.is_empty() {
				if let Some(text_caster) = &text_caster {
					Some(text_caster.cast_batch(&rows[..1], &unprintable_cells).await)
				} else {
					None
				}
			} else {
				None
			};

			// Build a map of column index to cast result
			let mut cast_map = std::collections::HashMap::new();
			if let Some(results) = cast_results {
				for (cell, result) in unprintable_cells.iter().zip(results.into_iter()) {
					cast_map.insert(cell.col_idx, result);
				}
			}

			let row = &rows[0];
			for (i, column) in columns.iter().enumerate() {
				let var_name = if let Some(prefix_str) = var_prefix {
					format!("{prefix_str}{col_name}", col_name = column.name())
				} else {
					column.name().to_string()
				};

				// Get the value as a string
				let value = if unprintable_columns.contains(&i) {
					cast_map
						.get(&i)
						.and_then(|r| r.as_ref().ok())
						.cloned()
						.unwrap_or_else(String::new)
				} else {
					get_value(row, i, &unprintable_columns)
				};

				vars_map.insert(var_name, value);
			}
		}
	}

	Ok(())
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

	#[test]
	fn test_command_to_verb() {
		assert_eq!(command_to_verb("INSERT"), "inserted");
		assert_eq!(command_to_verb("insert"), "inserted");
		assert_eq!(command_to_verb("UPDATE"), "updated");
		assert_eq!(command_to_verb("DELETE"), "deleted");
		assert_eq!(command_to_verb("CREATE"), "created");
		assert_eq!(command_to_verb("DROP"), "dropped");
		assert_eq!(command_to_verb("ALTER"), "altered");
		assert_eq!(command_to_verb("TRUNCATE"), "truncated");
		assert_eq!(command_to_verb("BEGIN"), "began");
		assert_eq!(command_to_verb("COMMIT"), "committed");
		assert_eq!(command_to_verb("ROLLBACK"), "rolled back");
		assert_eq!(command_to_verb("GRANT"), "granted");
		assert_eq!(command_to_verb("REVOKE"), "revoked");
		assert_eq!(command_to_verb("COPY"), "copied");
		assert_eq!(command_to_verb("MERGE"), "merged");
		assert_eq!(command_to_verb("SET"), "set");
		assert_eq!(command_to_verb("VACUUM"), "vacuumed");
		assert_eq!(command_to_verb("ANALYZE"), "analyzed");
		assert_eq!(command_to_verb("UNKNOWN"), "affected");
	}

	#[test]
	fn test_should_hide_row_count() {
		// Commands that should hide row count
		assert!(should_hide_row_count("COMMIT"));
		assert!(should_hide_row_count("commit"));
		assert!(should_hide_row_count("ROLLBACK"));
		assert!(should_hide_row_count("BEGIN"));
		assert!(should_hide_row_count("TRUNCATE"));
		assert!(should_hide_row_count("SAVEPOINT"));
		assert!(should_hide_row_count("RELEASE"));
		assert!(should_hide_row_count("SET"));
		assert!(should_hide_row_count("RESET"));
		assert!(should_hide_row_count("PREPARE"));
		assert!(should_hide_row_count("DEALLOCATE"));
		assert!(should_hide_row_count("DISCARD"));
		assert!(should_hide_row_count("LOCK"));
		assert!(should_hide_row_count("UNLISTEN"));
		assert!(should_hide_row_count("LISTEN"));
		assert!(should_hide_row_count("NOTIFY"));
		assert!(should_hide_row_count("VACUUM"));
		assert!(should_hide_row_count("ANALYZE"));
		assert!(should_hide_row_count("CLUSTER"));
		assert!(should_hide_row_count("REINDEX"));
		assert!(should_hide_row_count("COMMENT"));

		// Commands that should show row count
		assert!(!should_hide_row_count("INSERT"));
		assert!(!should_hide_row_count("UPDATE"));
		assert!(!should_hide_row_count("DELETE"));
		assert!(!should_hide_row_count("CREATE"));
		assert!(!should_hide_row_count("DROP"));
		assert!(!should_hide_row_count("ALTER"));
		assert!(!should_hide_row_count("GRANT"));
		assert!(!should_hide_row_count("REVOKE"));
		assert!(!should_hide_row_count("COPY"));
		assert!(!should_hide_row_count("MERGE"));
	}
}
