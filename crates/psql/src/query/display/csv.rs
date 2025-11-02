use miette::{IntoDiagnostic, Result};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::query::column;

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

	// Write rows
	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut record = Vec::new();
		for &i in &column_indices {
			let value_str =
				column::get_value(row, i, row_idx, ctx.unprintable_columns, ctx.text_rows);
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

		let pool = crate::pool::create_pool(&connection_string)
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
			columns,
			rows: &rows,
			unprintable_columns: &[],
			text_rows: &None,
			writer: &mut buffer,
			use_colours: false,
			theme: crate::theme::Theme::Dark,
			column_indices: None,
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

		let pool = crate::pool::create_pool(&connection_string)
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
			columns,
			rows: &rows,
			unprintable_columns: &[],
			text_rows: &None,
			writer: &mut buffer,
			use_colours: false,
			theme: crate::theme::Theme::Dark,
			column_indices: None,
		};

		display(&mut ctx).await.expect("Display failed");

		let output = String::from_utf8(buffer).expect("Invalid UTF-8");
		let lines: Vec<&str> = output.trim().lines().collect();

		assert_eq!(lines.len(), 2);
		assert_eq!(lines[0], "id,nullable,text");
		assert_eq!(lines[1], "1,NULL,value");
	}
}
