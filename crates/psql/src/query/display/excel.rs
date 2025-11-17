use miette::{IntoDiagnostic, Result};
use rust_xlsxwriter::Workbook;
use std::path::Path;

use crate::query::column;
use crate::query::text_cast::CellRef;

pub async fn display(
	ctx: &mut super::DisplayContext<'_, impl tokio::io::AsyncWrite + Unpin>,
	file_path: &str,
) -> Result<()> {
	if Path::new(file_path).exists() {
		return Err(miette::miette!("File already exists"));
	}

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

	// Collect all unprintable cells first
	let mut unprintable_cells = Vec::new();
	for (row_idx, _row) in ctx.rows.iter().enumerate() {
		for &col_idx in &column_indices {
			if ctx.unprintable_columns.contains(&col_idx) {
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

	// Write data rows
	for (row_idx, row) in ctx.rows.iter().enumerate() {
		for (col_idx, &i) in column_indices.iter().enumerate() {
			let value_str = if ctx.unprintable_columns.contains(&i) {
				let cell_ref = CellRef {
					row_idx,
					col_idx: i,
				};
				if let Some(result) = cast_map.get(&cell_ref) {
					match result {
						Ok(text) => text.clone(),
						Err(_) => "(error)".to_string(),
					}
				} else {
					"(binary data)".to_string()
				}
			} else {
				column::get_value(row, i, ctx.unprintable_columns)
			};

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
		drop(temp_file); // Delete the temp file so the path doesn't exist

		let mut ctx = crate::query::display::DisplayContext {
			columns,
			rows: &rows,
			unprintable_columns: &[],
			text_caster: None,
			writer: &mut buffer,
			use_colours: false,
			theme: crate::theme::Theme::Dark,
			column_indices: None,
		};

		display(&mut ctx, &file_path).await.expect("Display failed");

		// Verify the file was created and is a valid Excel file
		assert!(std::path::Path::new(&file_path).exists());
		let metadata = std::fs::metadata(&file_path).unwrap();
		assert!(metadata.len() > 0);
	}
}
