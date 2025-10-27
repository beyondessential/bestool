use crate::parser::QueryModifiers;
use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets, Attribute, Cell, CellAlignment, Table};
use miette::{IntoDiagnostic, Result};
use supports_unicode::Stream;
use tracing::debug;

pub(crate) async fn execute_query(
	client: &tokio_postgres::Client,
	sql: &str,
	modifiers: QueryModifiers,
) -> Result<()> {
	debug!(
		"executing query: {} (expanded={}, json={}, varset={}, prefix={:?})",
		sql, modifiers.expanded, modifiers.json, modifiers.varset, modifiers.prefix
	);

	let start = std::time::Instant::now();
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
			if !can_print_column(&first_row, i) {
				unprintable_columns.push(i);
			}
		}

		let text_rows = if !unprintable_columns.is_empty() {
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

fn supports_unicode() -> bool {
	supports_unicode::on(Stream::Stdout)
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
}
