use indexmap::IndexMap;
use miette::{IntoDiagnostic, Result};
use serde_json::Value;
use syntect::{
	easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet,
	util::as_24_bit_terminal_escaped,
};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::query::column;

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

	let mut objects = Vec::new();

	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut obj = IndexMap::new();
		for &i in &column_indices {
			let column = &ctx.columns[i];
			let value_str =
				column::get_value(row, i, row_idx, ctx.unprintable_columns, ctx.text_rows);

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

	let theme_name = match ctx.theme {
		crate::theme::Theme::Light => "base16-ocean.light",
		crate::theme::Theme::Dark => "base16-ocean.dark",
		crate::theme::Theme::Auto => "base16-ocean.dark",
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
