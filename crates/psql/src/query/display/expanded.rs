use bestool_postgres::{stringify::get_value, text_cast::CellRef};
use comfy_table::{Attribute, Cell, ColumnConstraint, Table, Width};
use miette::{IntoDiagnostic, Result};
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub async fn display<W: AsyncWrite + Unpin>(ctx: &mut super::DisplayContext<'_, W>) -> Result<()> {
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

	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let header = format!("-[ RECORD {num} ]-\n", num = row_idx + 1);
		ctx.writer
			.write_all(header.as_bytes())
			.await
			.into_diagnostic()?;

		let mut table = Table::new();
		crate::table::configure(&mut table);

		// No header in expanded mode, just column-value pairs
		for &col_idx in &column_indices {
			let column = &ctx.columns[col_idx];

			let name_cell = if ctx.use_colours {
				Cell::new(column.name()).add_attribute(Attribute::Bold)
			} else {
				Cell::new(column.name())
			};

			let value_cell = if ctx.should_redact(col_idx) {
				let cell = Cell::new(crate::colors::REDACTED_VALUE);
				if ctx.use_colours {
					cell.fg(crate::colors::to_comfy_color(
						crate::colors::Colors::REDACTED,
					))
				} else {
					cell
				}
			} else {
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
					get_value(row, col_idx, ctx.unprintable_columns)
				};
				Cell::new(value_str)
			};

			table.add_row(vec![name_cell, value_cell]);
		}

		// Set column constraints: fixed width for column names, flexible for values
		if let Some(col) = table.column_mut(0) {
			col.set_constraint(ColumnConstraint::ContentWidth);
		}
		if let Some(col) = table.column_mut(1)
			&& let Ok((width, _)) = crossterm::terminal::size()
		{
			// Reserve space for column name, borders, and padding
			let max_value_width = width.saturating_sub(20).max(30);
			col.set_constraint(ColumnConstraint::UpperBoundary(Width::Fixed(
				max_value_width,
			)));
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
