use comfy_table::Table;
use miette::{IntoDiagnostic, Result};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::query::column;
use crate::query::text_cast::CellRef;

pub async fn display<W: AsyncWrite + Unpin>(ctx: &mut super::DisplayContext<'_, W>) -> Result<()> {
	let mut table = Table::new();
	crate::table::configure(&mut table);

	// Determine which columns to display
	let column_indices: Vec<usize> = if let Some(indices) = ctx.column_indices {
		indices.to_vec()
	} else {
		(0..ctx.columns.len()).collect()
	};

	// Set header with filtered columns
	table.set_header(column_indices.iter().map(|&i| ctx.columns[i].name()));
	crate::table::style_header(&mut table);

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

	// Now build the table with all values
	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut row_data = Vec::new();
		for &col_idx in &column_indices {
			let value_str = if ctx.unprintable_columns.contains(&col_idx) {
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
				column::get_value(row, col_idx, ctx.unprintable_columns)
			};
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
