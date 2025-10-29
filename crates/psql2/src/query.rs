use crate::parser::{QueryModifier, QueryModifiers};
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
use tracing::{debug, warn};

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::sync::Arc;

/// Execute a SQL query and display the results.
pub(crate) async fn execute_query(
	client: &tokio_postgres::Client,
	sql: &str,
	modifiers: QueryModifiers,
	theme: crate::highlighter::Theme,
) -> Result<()> {
	debug!(?modifiers, %sql, "executing query");

	let start = std::time::Instant::now();

	#[cfg(unix)]
	let rows = {
		use tokio::signal::unix::{signal, SignalKind};

		// Block SIGINT at the signal mask level to prevent rustyline's handler from running
		// We'll handle it ourselves with tokio's signal handler
		let mut old_mask = unsafe { std::mem::zeroed() };
		let mut new_mask = unsafe { std::mem::zeroed() };
		unsafe {
			libc::sigemptyset(&mut new_mask);
			libc::sigaddset(&mut new_mask, libc::SIGINT);
			libc::pthread_sigmask(libc::SIG_BLOCK, &new_mask, &mut old_mask);
		}

		// Set up our own signal handler for SIGINT
		let mut sigint = signal(SignalKind::interrupt()).into_diagnostic()?;
		let cancel_token = client.cancel_token();
		let tls_connector = crate::tls::make_tls_connector()?;
		let cancelled = Arc::new(AtomicBool::new(false));
		let cancelled_clone = cancelled.clone();

		// Race between query execution and SIGINT
		let result = tokio::select! {
			result = client.query(sql, &[]) => {
				result.into_diagnostic()
			}
			_ = sigint.recv() => {
				cancelled_clone.store(true, Ordering::SeqCst);
				eprintln!("\nCancelling query...");
				if let Err(e) = cancel_token.cancel_query(tls_connector).await {
					warn!("Failed to cancel query: {:?}", e);
				}
				// Restore signal mask before returning
				unsafe {
					libc::pthread_sigmask(libc::SIG_SETMASK, &old_mask, std::ptr::null_mut());
				}
				return Ok(());
			}
		};

		// Restore the original signal mask
		unsafe {
			libc::pthread_sigmask(libc::SIG_SETMASK, &old_mask, std::ptr::null_mut());
		}

		result?
	};

	#[cfg(not(unix))]
	let rows = client.query(sql, &[]).await.into_diagnostic()?;

	let duration = start.elapsed();

	if rows.is_empty() {
		println!("(no rows)");
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
			let sql_trimmed = sql.trim_end_matches(';').trim();
			let text_query = build_text_cast_query(sql_trimmed, columns, &unprintable_columns);
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

		let is_expanded = modifiers.contains(&QueryModifier::Expanded);
		let is_json = modifiers.contains(&QueryModifier::Json);

		if is_json {
			display_json(
				columns,
				&rows,
				&unprintable_columns,
				&text_rows,
				is_expanded,
				theme,
			);
		} else if is_expanded {
			display_expanded(columns, &rows, &unprintable_columns, &text_rows);
		} else {
			display_normal(columns, &rows, &unprintable_columns, &text_rows);
		}

		eprintln!(
			"({} row{}, took {:.3}ms)",
			rows.len(),
			if rows.len() == 1 { "" } else { "s" },
			duration.as_secs_f64() * 1000.0
		);
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

fn display_expanded(
	columns: &[tokio_postgres::Column],
	rows: &[tokio_postgres::Row],
	unprintable_columns: &[usize],
	text_rows: &Option<Vec<tokio_postgres::Row>>,
) {
	for (row_idx, row) in rows.iter().enumerate() {
		println!("-[ RECORD {} ]-", row_idx + 1);

		let mut table = Table::new();

		if supports_unicode() {
			table.load_preset(presets::UTF8_FULL);
			table.apply_modifier(UTF8_ROUND_CORNERS);
		} else {
			table.load_preset(presets::ASCII_FULL);
		}

		table.set_content_arrangement(ContentArrangement::Dynamic);

		if let Some(width) = get_terminal_width() {
			table.set_width(width);
		}

		// No header in expanded mode, just column-value pairs
		for (i, column) in columns.iter().enumerate() {
			let value_str = get_column_value(row, i, row_idx, unprintable_columns, text_rows);

			table.add_row(vec![
				Cell::new(column.name()).add_attribute(Attribute::Bold),
				Cell::new(value_str),
			]);
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

		println!("{table}");
	}
}

fn display_normal(
	columns: &[tokio_postgres::Column],
	rows: &[tokio_postgres::Row],
	unprintable_columns: &[usize],
	text_rows: &Option<Vec<tokio_postgres::Row>>,
) {
	let mut table = Table::new();

	if supports_unicode() {
		table.load_preset(presets::UTF8_FULL);
		table.apply_modifier(UTF8_ROUND_CORNERS);
	} else {
		table.load_preset(presets::ASCII_FULL);
	}

	table.set_content_arrangement(ContentArrangement::Dynamic);

	if let Some(width) = get_terminal_width() {
		table.set_width(width);
	}

	table.set_header(columns.iter().map(|col| {
		Cell::new(col.name())
			.add_attribute(Attribute::Bold)
			.set_alignment(CellAlignment::Center)
	}));

	for (row_idx, row) in rows.iter().enumerate() {
		let mut row_data = Vec::new();
		for (i, _column) in columns.iter().enumerate() {
			let value_str = get_column_value(row, i, row_idx, unprintable_columns, text_rows);
			row_data.push(value_str);
		}
		table.add_row(row_data);
	}

	println!("{table}");
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

fn display_json(
	columns: &[tokio_postgres::Column],
	rows: &[tokio_postgres::Row],
	unprintable_columns: &[usize],
	text_rows: &Option<Vec<tokio_postgres::Row>>,
	expanded: bool,
	theme: crate::highlighter::Theme,
) {
	let mut objects = Vec::new();

	for (row_idx, row) in rows.iter().enumerate() {
		let mut obj = Map::new();

		for (i, column) in columns.iter().enumerate() {
			let value_str = get_column_value(row, i, row_idx, unprintable_columns, text_rows);

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

	let theme_name = match theme {
		crate::highlighter::Theme::Light => "base16-ocean.light",
		crate::highlighter::Theme::Dark => "base16-ocean.dark",
		crate::highlighter::Theme::Auto => "base16-ocean.dark",
	};

	let theme_obj = &theme_set.themes[theme_name];

	if expanded {
		// Pretty-print a single array containing all objects
		let json_str = serde_json::to_string_pretty(&objects).unwrap();
		let highlighted = highlight_json(&json_str, syntax, theme_obj, &syntax_set);
		println!("{}", highlighted);
	} else {
		// Compact-print one object per line
		for obj in objects {
			let json_str = serde_json::to_string(&obj).unwrap();
			let highlighted = highlight_json(&json_str, syntax, theme_obj, &syntax_set);
			println!("{}", highlighted);
		}
	}
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

fn supports_unicode() -> bool {
	supports_unicode::on(Stream::Stdout)
}

fn get_terminal_width() -> Option<u16> {
	crossterm::terminal::size().ok().map(|(w, _)| w)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_supports_unicode() {
		let _ = supports_unicode();
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
}
