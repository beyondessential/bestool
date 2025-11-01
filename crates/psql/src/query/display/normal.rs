use comfy_table::Table;
use miette::{IntoDiagnostic, Result};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::query::column;

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

	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut row_data = Vec::new();
		for &i in &column_indices {
			let value_str =
				column::get_value(row, i, row_idx, ctx.unprintable_columns, ctx.text_rows);
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
