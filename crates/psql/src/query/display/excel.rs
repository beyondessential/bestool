use miette::{IntoDiagnostic, Result};
use rust_xlsxwriter::Workbook;
use std::path::Path;

use crate::query::column;

pub async fn display(
	ctx: &super::DisplayContext<'_, impl tokio::io::AsyncWrite + Unpin>,
	file_path: &str,
) -> Result<()> {
	// Determine which columns to display
	let column_indices: Vec<usize> = if let Some(indices) = ctx.column_indices {
		indices.to_vec()
	} else {
		(0..ctx.columns.len()).collect()
	};

	let mut workbook = Workbook::new();
	let worksheet = workbook.add_worksheet();
	worksheet.set_name("Results").into_diagnostic()?;

	// Write header row
	for (col_idx, &i) in column_indices.iter().enumerate() {
		let column_name = ctx.columns[i].name();
		worksheet
			.write_string(0, col_idx as u16, column_name)
			.into_diagnostic()?;
	}

	// Write data rows
	for (row_idx, row) in ctx.rows.iter().enumerate() {
		for (col_idx, &i) in column_indices.iter().enumerate() {
			let value_str =
				column::get_value(row, i, row_idx, ctx.unprintable_columns, ctx.text_rows);

			// Excel row numbers are 1-based after the header
			let excel_row = (row_idx + 1) as u32;
			let excel_col = col_idx as u16;

			// Try to write as number if possible, otherwise write as string
			if value_str == "NULL" {
				// Write empty cell for NULL
				continue;
			} else if let Ok(num) = value_str.parse::<f64>() {
				worksheet
					.write_number(excel_row, excel_col, num)
					.into_diagnostic()?;
			} else if let Ok(num) = value_str.parse::<i64>() {
				worksheet
					.write_number(excel_row, excel_col, num as f64)
					.into_diagnostic()?;
			} else if value_str == "true" {
				worksheet
					.write_boolean(excel_row, excel_col, true)
					.into_diagnostic()?;
			} else if value_str == "false" {
				worksheet
					.write_boolean(excel_row, excel_col, false)
					.into_diagnostic()?;
			} else {
				worksheet
					.write_string(excel_row, excel_col, &value_str)
					.into_diagnostic()?;
			}
		}
	}

	// Auto-fit columns for better readability
	worksheet.autofit();

	workbook.save(Path::new(file_path)).into_diagnostic()?;

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_excel_display() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let rows = client
			.query(
				"SELECT 1 as id, 'Alice' as name, 25 as age, true as active, NULL as notes",
				&[],
			)
			.await
			.expect("Query failed");

		let columns = rows[0].columns();
		let mut buffer = Vec::new();

		let temp_file = tempfile::NamedTempFile::new().unwrap();
		let file_path = temp_file.path().to_string_lossy().to_string();

		let ctx = crate::query::display::DisplayContext {
			columns,
			rows: &rows,
			unprintable_columns: &[],
			text_rows: &None,
			writer: &mut buffer,
			use_colours: false,
			theme: crate::theme::Theme::Dark,
			column_indices: None,
		};

		display(&ctx, &file_path).await.expect("Display failed");

		// Verify the file was created and is a valid Excel file
		assert!(std::path::Path::new(&file_path).exists());
		let metadata = std::fs::metadata(&file_path).unwrap();
		assert!(metadata.len() > 0);
	}
}
