use comfy_table::{Attribute, Cell, ColumnConstraint, Table, Width};
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

	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let header = format!("-[ RECORD {num} ]-\n", num = row_idx + 1);
		ctx.writer
			.write_all(header.as_bytes())
			.await
			.into_diagnostic()?;

		let mut table = Table::new();
		crate::table::configure(&mut table);

		// No header in expanded mode, just column-value pairs
		for &i in &column_indices {
			let column = &ctx.columns[i];
			let value_str =
				column::get_value(row, i, row_idx, ctx.unprintable_columns, ctx.text_rows);

			let name_cell = if ctx.use_colours {
				Cell::new(column.name()).add_attribute(Attribute::Bold)
			} else {
				Cell::new(column.name())
			};

			table.add_row(vec![name_cell, Cell::new(value_str)]);
		}

		// Set column constraints: fixed width for column names, flexible for values
		if let Some(col) = table.column_mut(0) {
			col.set_constraint(ColumnConstraint::ContentWidth);
		}
		if let Some(col) = table.column_mut(1) {
			if let Ok((width, _)) = crossterm::terminal::size() {
				// Reserve space for column name, borders, and padding
				let max_value_width = width.saturating_sub(20).max(30);
				col.set_constraint(ColumnConstraint::UpperBoundary(Width::Fixed(
					max_value_width,
				)));
			}
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
