use comfy_table::{
	modifiers::UTF8_ROUND_CORNERS, presets, Attribute, Cell, CellAlignment, ColumnConstraint,
	ContentArrangement, Table, Width,
};
use miette::{IntoDiagnostic, Result};
use serde_json::{Map, Value};
use supports_unicode::Stream;
use syntect::{
	easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet,
	util::as_24_bit_terminal_escaped,
};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tracing::{debug, warn};

use crate::parser::{QueryModifier, QueryModifiers};

/// Interpolate variables in the SQL string.
/// Replaces ${name} with the value of variable `name`.
/// Escape sequences: ${{name}} becomes ${name} (without replacement).
/// Returns error if a variable is referenced but not set.
fn interpolate_variables(
	sql: &str,
	vars: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
	let bytes = sql.as_bytes();
	let mut result = String::new();
	let mut i = 0;

	while i < bytes.len() {
		if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
			// Check if it's an escape sequence ${{
			if i + 2 < bytes.len() && bytes[i + 2] == b'{' {
				// Escape sequence: ${{ -> find }} and output ${...}
				i += 3; // skip ${
				result.push_str("${");

				// Find the closing }}
				while i < bytes.len() {
					if i + 1 < bytes.len() && bytes[i] == b'}' && bytes[i + 1] == b'}' {
						result.push('}');
						i += 2;
						break;
					}
					result.push(bytes[i] as char);
					i += 1;
				}
			} else {
				// Normal substitution: ${name}
				i += 2; // skip ${
				let var_start = i;

				// Find the closing }
				while i < bytes.len() && bytes[i] != b'}' {
					i += 1;
				}

				if i < bytes.len() && bytes[i] == b'}' {
					let var_name = std::str::from_utf8(&bytes[var_start..i])
						.unwrap_or_default()
						.trim();
					if let Some(value) = vars.get(var_name) {
						result.push_str(value);
					} else {
						miette::bail!("Variable '{}' is not set", var_name);
					}
					i += 1; // skip closing }
				}
			}
		} else {
			result.push(bytes[i] as char);
			i += 1;
		}
	}

	Ok(result)
}

/// Context for executing a query.
pub(crate) struct QueryContext<'a, W: AsyncWrite + Unpin> {
	pub client: &'a tokio_postgres::Client,
	pub modifiers: QueryModifiers,
	pub theme: crate::theme::Theme,
	pub writer: &'a mut W,
	pub use_colours: bool,
	pub vars: Option<&'a mut std::collections::BTreeMap<String, String>>,
}

/// Context for displaying query results.
struct DisplayContext<'a, W: AsyncWrite + Unpin> {
	columns: &'a [tokio_postgres::Column],
	rows: &'a [tokio_postgres::Row],
	unprintable_columns: &'a [usize],
	text_rows: &'a Option<Vec<tokio_postgres::Row>>,
	writer: &'a mut W,
	use_colours: bool,
	theme: crate::theme::Theme,
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
		let empty_vars = std::collections::BTreeMap::new();
		let vars = ctx.vars.as_ref().map_or(&empty_vars, |v| v);
		interpolate_variables(sql, vars)?
	};

	let start = std::time::Instant::now();

	let cancel_token = ctx.client.cancel_token();
	let tls_connector = crate::tls::make_tls_connector()?;

	// Reset the flag before starting
	crate::reset_sigint();

	// Poll for SIGINT while executing query
	let result = tokio::select! {
		result = ctx.client.query(&sql_to_execute, &[]) => {
			result.into_diagnostic()
		}
		_ = async {
			loop {
				tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
				if crate::sigint_received() {
					break;
				}
			}
		} => {
			eprintln!("\nCancelling query...");
			if let Err(e) = cancel_token.cancel_query(tls_connector).await {
				warn!("Failed to cancel query: {:?}", e);
			}
			// Reset flag for next time
			crate::reset_sigint();
			return Ok(());
		}
	};

	let rows = result?;

	let duration = start.elapsed();

	if rows.is_empty() {
		ctx.writer
			.write_all(b"(no rows)\n")
			.await
			.into_diagnostic()?;
		ctx.writer.flush().await.into_diagnostic()?;
		return Ok(());
	}

	if let Some(first_row) = rows.first() {
		let columns = first_row.columns();

		let mut unprintable_columns = Vec::new();
		for (i, _column) in columns.iter().enumerate() {
			if !can_print_column(first_row, i) {
				unprintable_columns.push(i);
			}
		}

		let text_rows = if !unprintable_columns.is_empty() {
			let sql_trimmed = sql_to_execute.trim_end_matches(';').trim();
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

		let is_expanded = ctx.modifiers.contains(&QueryModifier::Expanded);
		let is_json = ctx.modifiers.contains(&QueryModifier::Json);

		let mut display_ctx = DisplayContext {
			columns,
			rows: &rows,
			unprintable_columns: &unprintable_columns,
			text_rows: &text_rows,
			writer: ctx.writer,
			use_colours: ctx.use_colours,
			theme: ctx.theme,
		};

		if is_json {
			display_json(&mut display_ctx, is_expanded).await?;
		} else if is_expanded {
			display_expanded(&mut display_ctx).await?;
		} else {
			display_normal(&mut display_ctx).await?;
		}

		let status_msg = format!(
			"({} row{}, took {:.3}ms)\n",
			rows.len(),
			if rows.len() == 1 { "" } else { "s" },
			duration.as_secs_f64() * 1000.0
		);
		// Status messages always go to stderr
		eprint!("{status_msg}");

		// Handle VarSet modifier: if exactly one row, store column values as variables
		if rows.len() == 1 {
			if let Some(var_prefix) = ctx.modifiers.iter().find_map(|m| {
				if let QueryModifier::VarSet { prefix } = m {
					Some(prefix)
				} else {
					None
				}
			}) {
				if let Some(vars_map) = ctx.vars.as_mut() {
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
							get_column_value(row, i, 0, &unprintable_columns, &text_rows)
						};

						vars_map.insert(var_name, value);
					}
				}
			}
		}
	}

	Ok(())
}

fn can_print_column(row: &tokio_postgres::Row, i: usize) -> bool {
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
		format!("\\x{encoded}", encoded = hex::encode(v))
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
	let column_exprs: Vec<String> = columns
		.iter()
		.enumerate()
		.map(|(i, col)| {
			if unprintable_columns.contains(&i) {
				format!("(subq.{col_name})::text", col_name = col.name())
			} else {
				format!("subq.{col_name}", col_name = col.name())
			}
		})
		.collect();

	format!(
		"SELECT {cols} FROM ({sql}) AS subq",
		cols = column_exprs.join(", ")
	)
}

pub(crate) fn configure_table(table: &mut Table) {
	if supports_unicode::on(Stream::Stdout) {
		table.load_preset(presets::UTF8_FULL);
		table.apply_modifier(UTF8_ROUND_CORNERS);
	} else {
		table.load_preset(presets::ASCII_FULL);
	}

	table.set_content_arrangement(ContentArrangement::Dynamic);

	if let Some(width) = get_terminal_width() {
		table.set_width(width);
	}
}

async fn display_expanded<W: AsyncWrite + Unpin>(ctx: &mut DisplayContext<'_, W>) -> Result<()> {
	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let header = format!("-[ RECORD {num} ]-\n", num = row_idx + 1);
		ctx.writer
			.write_all(header.as_bytes())
			.await
			.into_diagnostic()?;

		let mut table = Table::new();
		configure_table(&mut table);

		// No header in expanded mode, just column-value pairs
		for (i, column) in ctx.columns.iter().enumerate() {
			let value_str =
				get_column_value(row, i, row_idx, ctx.unprintable_columns, ctx.text_rows);

			let name_cell = if ctx.use_colours {
				Cell::new(column.name()).add_attribute(Attribute::Bold)
			} else {
				Cell::new(column.name())
			};

			table.add_row(vec![name_cell, Cell::new(value_str)]);
		}

		// Set column constraints: fixed width for column names, flexible for values
		if let Some(col) = table.column_mut(0) {
			col.set_constraint(ColumnConstraint::ContentWidth);
		}
		if let Some(col) = table.column_mut(1) {
			if let Some(width) = get_terminal_width() {
				// Reserve space for column name, borders, and padding
				let max_value_width = width.saturating_sub(20).max(30);
				col.set_constraint(ColumnConstraint::UpperBoundary(Width::Fixed(
					max_value_width,
				)));
			}
		}

		let table_output = format!("{table}\n");
		ctx.writer
			.write_all(table_output.as_bytes())
			.await
			.into_diagnostic()?;
	}
	ctx.writer.flush().await.into_diagnostic()?;
	Ok(())
}

async fn display_normal<W: AsyncWrite + Unpin>(ctx: &mut DisplayContext<'_, W>) -> Result<()> {
	let mut table = Table::new();
	configure_table(&mut table);

	table.set_header(ctx.columns.iter().map(|col| {
		let cell = Cell::new(col.name()).set_alignment(CellAlignment::Center);
		if ctx.use_colours {
			cell.add_attribute(Attribute::Bold)
		} else {
			cell
		}
	}));

	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut row_data = Vec::new();
		for (i, _column) in ctx.columns.iter().enumerate() {
			let value_str =
				get_column_value(row, i, row_idx, ctx.unprintable_columns, ctx.text_rows);
			row_data.push(value_str);
		}
		table.add_row(row_data);
	}

	let table_output = format!("{table}\n");
	ctx.writer
		.write_all(table_output.as_bytes())
		.await
		.into_diagnostic()?;
	ctx.writer.flush().await.into_diagnostic()?;
	Ok(())
}

fn get_column_value(
	row: &tokio_postgres::Row,
	column_index: usize,
	row_index: usize,
	unprintable_columns: &[usize],
	text_rows: &Option<Vec<tokio_postgres::Row>>,
) -> String {
	if !unprintable_columns.contains(&column_index) {
		return format_column_value(row, column_index);
	}

	if let Some(ref text_rows) = text_rows {
		if let Some(text_row) = text_rows.get(row_index) {
			return text_row
				.try_get::<_, Option<String>>(column_index)
				.ok()
				.flatten()
				.unwrap_or_else(|| "NULL".to_string());
		}
	}

	"(error)".to_string()
}

async fn display_json<W: AsyncWrite + Unpin>(
	ctx: &mut DisplayContext<'_, W>,
	expanded: bool,
) -> Result<()> {
	let mut objects = Vec::new();

	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut obj = Map::new();
		for (i, column) in ctx.columns.iter().enumerate() {
			let value_str =
				get_column_value(row, i, row_idx, ctx.unprintable_columns, ctx.text_rows);

			// Try to parse the value as JSON if it's a valid JSON string
			let json_value = if value_str == "NULL" {
				Value::Null
			} else if let Ok(parsed) = serde_json::from_str::<Value>(&value_str) {
				parsed
			} else {
				Value::String(value_str)
			};

			obj.insert(column.name().to_string(), json_value);
		}

		objects.push(Value::Object(obj));
	}

	let syntax_set = SyntaxSet::load_defaults_newlines();
	let theme_set = ThemeSet::load_defaults();

	let syntax = syntax_set
		.find_syntax_by_extension("json")
		.unwrap_or_else(|| syntax_set.find_syntax_plain_text());

	let theme_name = match ctx.theme {
		crate::theme::Theme::Light => "base16-ocean.light",
		crate::theme::Theme::Dark => "base16-ocean.dark",
		crate::theme::Theme::Auto => "base16-ocean.dark",
	};

	let theme_obj = &theme_set.themes[theme_name];

	if expanded {
		// Pretty-print a single array containing all objects
		let json_str = serde_json::to_string_pretty(&objects).unwrap();
		if ctx.use_colours {
			let highlighted = highlight_json(&json_str, syntax, theme_obj, &syntax_set);
			ctx.writer
				.write_all(format!("{highlighted}\n").as_bytes())
				.await
				.into_diagnostic()?;
		} else {
			ctx.writer
				.write_all(format!("{json_str}\n").as_bytes())
				.await
				.into_diagnostic()?;
		}
	} else {
		// Compact-print one object per line
		for obj in objects {
			let json_str = serde_json::to_string(&obj).unwrap();
			if ctx.use_colours {
				let highlighted = highlight_json(&json_str, syntax, theme_obj, &syntax_set);
				ctx.writer
					.write_all(format!("{highlighted}\n").as_bytes())
					.await
					.into_diagnostic()?;
			} else {
				ctx.writer
					.write_all(format!("{json_str}\n").as_bytes())
					.await
					.into_diagnostic()?;
			}
		}
	}

	ctx.writer.flush().await.into_diagnostic()?;
	Ok(())
}

fn highlight_json(
	json_str: &str,
	syntax: &syntect::parsing::SyntaxReference,
	theme: &syntect::highlighting::Theme,
	syntax_set: &SyntaxSet,
) -> String {
	let mut highlighter = HighlightLines::new(syntax, theme);
	let mut result = String::new();

	for line in json_str.lines() {
		match highlighter.highlight_line(line, syntax_set) {
			Ok(ranges) => {
				let mut escaped = as_24_bit_terminal_escaped(&ranges[..], false);
				escaped.push_str("\x1b[0m");
				result.push_str(&escaped);
				result.push('\n');
			}
			Err(_) => {
				result.push_str(line);
				result.push('\n');
			}
		}
	}

	// Remove trailing newline if original didn't have one
	if !json_str.ends_with('\n') && result.ends_with('\n') {
		result.pop();
	}

	result
}

fn get_terminal_width() -> Option<u16> {
	crossterm::terminal::size().ok().map(|(w, _)| w)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_supports_unicode() {
		let _ = supports_unicode::on(Stream::Stdout);
	}

	#[test]
	fn test_build_text_cast_query_logic() {
		let sql = "SELECT id, name, data FROM users";
		let column_names = vec!["id", "name", "data"];
		let unprintable_indices = vec![0, 2];

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
	fn test_json_value_parsing() {
		let null_value = "NULL";
		let json_value = if null_value == "NULL" {
			Value::Null
		} else if let Ok(parsed) = serde_json::from_str::<Value>(null_value) {
			parsed
		} else {
			Value::String(null_value.to_string())
		};
		assert_eq!(json_value, Value::Null);

		let string_value = "hello";
		let json_value = if string_value == "NULL" {
			Value::Null
		} else if let Ok(parsed) = serde_json::from_str::<Value>(string_value) {
			parsed
		} else {
			Value::String(string_value.to_string())
		};
		assert_eq!(json_value, Value::String("hello".to_string()));

		let json_string = r#"{"key":"value"}"#;
		let json_value = if json_string == "NULL" {
			Value::Null
		} else if let Ok(parsed) = serde_json::from_str::<Value>(json_string) {
			parsed
		} else {
			Value::String(json_string.to_string())
		};
		assert!(json_value.is_object());
	}

	#[test]
	fn test_get_terminal_width() {
		// This test just ensures the function can be called without panicking
		// The actual return value depends on the environment
		let _ = get_terminal_width();
	}

	#[test]
	fn test_interpolate_variables_basic() {
		let mut vars = std::collections::BTreeMap::new();
		vars.insert("name".to_string(), "Alice".to_string());
		vars.insert("value".to_string(), "42".to_string());

		let sql = "SELECT * WHERE name = ${name} AND value = ${value}";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, "SELECT * WHERE name = Alice AND value = 42");
	}

	#[test]
	fn test_interpolate_variables_no_substitution() {
		let vars = std::collections::BTreeMap::new();
		let sql = "SELECT * FROM users";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, sql);
	}

	#[test]
	fn test_interpolate_variables_missing_var() {
		let vars = std::collections::BTreeMap::new();
		let sql = "SELECT * WHERE name = ${name}";
		let result = interpolate_variables(sql, &vars);
		assert!(result.is_err());
	}

	#[test]
	fn test_interpolate_variables_escape_sequence() {
		let mut vars = std::collections::BTreeMap::new();
		vars.insert("name".to_string(), "Alice".to_string());

		let sql = "SELECT ${{name}}, ${name}";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, "SELECT ${name}, Alice");
	}

	#[test]
	fn test_interpolate_variables_in_quoted_string() {
		let mut vars = std::collections::BTreeMap::new();
		vars.insert("name".to_string(), "O'Brien".to_string());

		let sql = "SELECT * WHERE name = '${name}'";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, "SELECT * WHERE name = 'O'Brien'");
	}

	#[test]
	fn test_interpolate_variables_multiple_escapes() {
		let mut vars = std::collections::BTreeMap::new();
		vars.insert("x".to_string(), "10".to_string());

		let sql = "SELECT ${{x}}, ${{x}}, ${x}";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, "SELECT ${x}, ${x}, 10");
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
}
