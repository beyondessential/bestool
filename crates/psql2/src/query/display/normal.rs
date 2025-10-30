use comfy_table::{Attribute, Cell, CellAlignment, Table};
use miette::{IntoDiagnostic, Result};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::query::column;

pub async fn display<W: AsyncWrite + Unpin>(ctx: &mut super::DisplayContext<'_, W>) -> Result<()> {
	let mut table = Table::new();
	crate::table::configure(&mut table);

	table.set_header(ctx.columns.iter().map(|col| {
		let cell = Cell::new(col.name()).set_alignment(CellAlignment::Center);
		if ctx.use_colours {
			cell.add_attribute(Attribute::Bold)
		} else {
			cell
		}
	}));

	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut row_data = Vec::new();
		for (i, _column) in ctx.columns.iter().enumerate() {
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
