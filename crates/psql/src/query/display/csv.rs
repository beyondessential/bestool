use bestool_postgres::{stringify::get_value, text_cast::CellRef};
use miette::{IntoDiagnostic, Result};
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub async fn display<W: AsyncWrite + Unpin>(ctx: &mut super::DisplayContext<'_, W>) -> Result<()> {
	// Determine which columns to display
	let column_indices: Vec<usize> = if let Some(indices) = ctx.column_indices {
		indices.to_vec()
	} else {
		(0..ctx.columns.len()).collect()
	};

	// Create an in-memory buffer for the CSV writer
	let mut buffer = Vec::new();
	let mut writer = csv::Writer::from_writer(&mut buffer);

	// Write header
	let headers: Vec<&str> = column_indices
		.iter()
		.map(|&i| ctx.columns[i].name())
		.collect();
	writer.write_record(&headers).into_diagnostic()?;

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

	// Write rows
	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut record = Vec::new();
		for &col_idx in &column_indices {
			let value_str = if ctx.should_redact(col_idx) {
				ctx.redacted_value()
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
			record.push(value_str);
		}
		writer.write_record(&record).into_diagnostic()?;
	}

	writer.flush().into_diagnostic()?;
	drop(writer);

	// Write the buffer to the async writer
	ctx.writer.write_all(&buffer).await.into_diagnostic()?;
	ctx.writer.flush().await.into_diagnostic()?;

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_csv_display_with_escaping() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Test CSV escaping with quotes, commas, and newlines
		let rows = client
			.query(
				"SELECT 'test' as simple, 'with,comma' as has_comma, 'with\"quote' as has_quote, E'with\\nnewline' as has_newline",
				&[],
			)
			.await
			.expect("Query failed");

		let columns = rows[0].columns();
		let mut buffer = Vec::new();

		let mut ctx = crate::query::display::DisplayContext {
			config: &Default::default(),
			columns,
			rows: &rows,
			unprintable_columns: &[],
			text_caster: None,
			writer: &mut buffer,
			use_colours: false,
			column_indices: None,
			redact_mode: false,
			column_refs: &[],
		};

		display(&mut ctx).await.expect("Display failed");

		let output = String::from_utf8(buffer).expect("Invalid UTF-8");

		// Parse the CSV output properly
		let mut reader = csv::Reader::from_reader(output.as_bytes());
		let headers = reader.headers().expect("Failed to read headers");
		assert_eq!(headers.len(), 4);
		assert_eq!(&headers[0], "simple");
		assert_eq!(&headers[1], "has_comma");
		assert_eq!(&headers[2], "has_quote");
		assert_eq!(&headers[3], "has_newline");

		// Read the data row
		let mut records = reader.records();
		let record = records
			.next()
			.expect("Missing record")
			.expect("Failed to parse record");
		assert_eq!(&record[0], "test");
		assert_eq!(&record[1], "with,comma");
		assert_eq!(&record[2], "with\"quote");
		assert_eq!(&record[3], "with\nnewline");

		// Should only have one data row
		assert!(records.next().is_none());
	}

	#[tokio::test]
	async fn test_csv_display_with_nulls() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Test NULL values
		let rows = client
			.query("SELECT 1 as id, NULL as nullable, 'value' as text", &[])
			.await
			.expect("Query failed");

		let columns = rows[0].columns();
		let mut buffer = Vec::new();

		let mut ctx = crate::query::display::DisplayContext {
			config: &Default::default(),
			columns,
			rows: &rows,
			unprintable_columns: &[],
			text_caster: None,
			writer: &mut buffer,
			use_colours: false,
			column_indices: None,
			redact_mode: false,
			column_refs: &[],
		};

		display(&mut ctx).await.expect("Display failed");

		let output = String::from_utf8(buffer).expect("Invalid UTF-8");
		let lines: Vec<&str> = output.trim().lines().collect();

		assert_eq!(lines.len(), 2);
		assert_eq!(lines[0], "id,nullable,text");
		assert_eq!(lines[1], "1,NULL,value");
	}

	#[tokio::test]
	async fn test_csv_display_with_redaction() {
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

		display(&mut ctx).await.expect("Display failed");

		let output = String::from_utf8(buffer).expect("Invalid UTF-8");
		let lines: Vec<&str> = output.trim().lines().collect();

		assert_eq!(lines.len(), 2);
		assert_eq!(lines[0], "name,password,age");
		assert_eq!(lines[1], "Alice,[redacted],25");
	}
}
