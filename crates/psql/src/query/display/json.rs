use bestool_postgres::{stringify::get_value, text_cast::CellRef};
use indexmap::IndexMap;
use miette::{IntoDiagnostic, Result};
use serde_json::Value;
use syntect::{
	easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet,
	util::as_24_bit_terminal_escaped,
};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::colors::{self, REDACTED_VALUE};

pub async fn display<W: AsyncWrite + Unpin>(
	ctx: &mut super::DisplayContext<'_, W>,
	expanded: bool,
) -> Result<()> {
	// Determine which columns to display
	let column_indices: Vec<usize> = if let Some(indices) = ctx.column_indices {
		indices.to_vec()
	} else {
		(0..ctx.columns.len()).collect()
	};

	// Collect all unprintable cells first (excluding redacted ones)
	let mut unprintable_cells = Vec::new();
	for (row_idx, _row) in ctx.rows.iter().enumerate() {
		for &col_idx in &column_indices {
			if ctx.unprintable_columns.contains(&col_idx) && !ctx.should_redact(col_idx) {
				unprintable_cells.push(CellRef { row_idx, col_idx });
			}
		}
	}

	// Batch cast all unprintable cells if we have a text caster
	let cast_results = if !unprintable_cells.is_empty() {
		if let Some(text_caster) = &ctx.text_caster {
			Some(text_caster.cast_batch(ctx.rows, &unprintable_cells).await)
		} else {
			None
		}
	} else {
		None
	};

	// Build index for looking up cast results
	let mut cast_map = std::collections::HashMap::new();
	if let Some(results) = cast_results {
		for (cell, result) in unprintable_cells.iter().zip(results.into_iter()) {
			cast_map.insert(*cell, result);
		}
	}

	let mut objects = Vec::new();

	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut obj = IndexMap::new();
		for &col_idx in &column_indices {
			let column = &ctx.columns[col_idx];
			let value_str = if ctx.should_redact(col_idx) {
				REDACTED_VALUE.to_string()
			} else if ctx.unprintable_columns.contains(&col_idx) {
				let cell_ref = CellRef { row_idx, col_idx };
				if let Some(result) = cast_map.get(&cell_ref) {
					match result {
						Ok(text) => text.clone(),
						Err(_) => "(error)".to_string(),
					}
				} else {
					"(binary data)".to_string()
				}
			} else {
				get_value(row, col_idx, ctx.unprintable_columns)
			};

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

		objects.push(obj);
	}

	let syntax_set = SyntaxSet::load_defaults_newlines();
	let theme_set = ThemeSet::load_defaults();

	let syntax = syntax_set
		.find_syntax_by_extension("json")
		.unwrap_or_else(|| syntax_set.find_syntax_plain_text());

	let theme_name = match ctx.config.theme {
		crate::theme::Theme::Light => "base16-ocean.light",
		crate::theme::Theme::Dark | crate::theme::Theme::Auto => "base16-ocean.dark",
	};

	let theme_obj = &theme_set.themes[theme_name];

	if expanded {
		// Pretty-print a single array containing all objects
		let json_str = serde_json::to_string_pretty(&objects).into_diagnostic()?;
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
			let json_str = serde_json::to_string(&obj).into_diagnostic()?;
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
				escaped.push_str(&colors::reset_code());
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

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_json_display_with_redaction() {
		use std::collections::HashSet;
		use std::sync::Arc;

		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Test redaction
		let rows = client
			.query(
				"SELECT 'Alice' as name, 'secret123' as password, 25 as age",
				&[],
			)
			.await
			.expect("Query failed");

		let columns = rows[0].columns();
		let mut buffer = Vec::new();

		// Set up redaction for the password column
		let mut redactions = HashSet::new();
		redactions.insert(crate::column_extractor::ColumnRef {
			schema: "".to_string(),
			table: "".to_string(),
			column: "password".to_string(),
		});

		let config = Arc::new(crate::Config {
			redactions,
			..Default::default()
		});

		let column_refs = vec![
			crate::column_extractor::ColumnRef {
				schema: "".to_string(),
				table: "".to_string(),
				column: "name".to_string(),
			},
			crate::column_extractor::ColumnRef {
				schema: "".to_string(),
				table: "".to_string(),
				column: "password".to_string(),
			},
			crate::column_extractor::ColumnRef {
				schema: "".to_string(),
				table: "".to_string(),
				column: "age".to_string(),
			},
		];

		let mut ctx = crate::query::display::DisplayContext {
			config: &config,
			columns,
			rows: &rows,
			unprintable_columns: &[],
			text_caster: None,
			writer: &mut buffer,
			use_colours: false,
			column_indices: None,
			redact_mode: true,
			column_refs: &column_refs,
		};

		// Test compact JSON format
		display(&mut ctx, false).await.expect("Display failed");

		let output = String::from_utf8(buffer).expect("Invalid UTF-8");
		let lines: Vec<&str> = output.trim().lines().collect();

		assert_eq!(lines.len(), 1);
		let parsed: serde_json::Value =
			serde_json::from_str(lines[0]).expect("Should be valid JSON");
		assert_eq!(parsed["name"], "Alice");
		assert_eq!(parsed["password"], "[redacted]");
		assert_eq!(parsed["age"], 25);

		// Test expanded JSON format
		let mut buffer2 = Vec::new();
		let mut ctx2 = crate::query::display::DisplayContext {
			config: &config,
			columns,
			rows: &rows,
			unprintable_columns: &[],
			text_caster: None,
			writer: &mut buffer2,
			use_colours: false,
			column_indices: None,
			redact_mode: true,
			column_refs: &column_refs,
		};

		display(&mut ctx2, true).await.expect("Display failed");

		let output2 = String::from_utf8(buffer2).expect("Invalid UTF-8");
		let parsed2: serde_json::Value =
			serde_json::from_str(&output2.trim()).expect("Should be valid JSON array");
		assert!(parsed2.is_array());
		let array = parsed2.as_array().unwrap();
		assert_eq!(array.len(), 1);
		assert_eq!(array[0]["name"], "Alice");
		assert_eq!(array[0]["password"], "[redacted]");
		assert_eq!(array[0]["age"], 25);
	}
}
