use bestool_postgres::{stringify::get_value, text_cast::CellRef};
use miette::{IntoDiagnostic, Result};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::colors::REDACTED_VALUE;

/// Plain output: the first (selected) column of every row, one value per line, with no
/// header, borders, or quoting. Warns if more than one column would be shown.
pub async fn display<W: AsyncWrite + Unpin>(ctx: &mut super::DisplayContext<'_, W>) -> Result<()> {
	let column_indices: Vec<usize> = if let Some(indices) = ctx.column_indices {
		indices.to_vec()
	} else {
		(0..ctx.columns.len()).collect()
	};

	let Some(&col_idx) = column_indices.first() else {
		return Ok(());
	};

	if column_indices.len() > 1 {
		eprintln!(
			"warning: plain output shows only the first column ('{}'); {} other column(s) ignored",
			ctx.columns[col_idx].name(),
			column_indices.len() - 1
		);
	}

	// Cast unprintable cells in the target column up front.
	let mut cast_map = std::collections::HashMap::new();
	if ctx.unprintable_columns.contains(&col_idx)
		&& !ctx.should_redact(col_idx)
		&& let Some(text_caster) = &ctx.text_caster
	{
		let cells: Vec<CellRef> = (0..ctx.rows.len())
			.map(|row_idx| CellRef { row_idx, col_idx })
			.collect();
		let results = text_caster.cast_batch(ctx.rows, &cells).await;
		for (cell, result) in cells.into_iter().zip(results) {
			cast_map.insert(cell, result);
		}
	}

	let mut buffer = String::new();
	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let value_str = if ctx.should_redact(col_idx) {
			REDACTED_VALUE.to_string()
		} else if ctx.unprintable_columns.contains(&col_idx) {
			match cast_map.get(&CellRef { row_idx, col_idx }) {
				Some(Ok(text)) => text.clone(),
				Some(Err(_)) => "(error)".to_string(),
				None => "(binary data)".to_string(),
			}
		} else {
			get_value(row, col_idx, ctx.unprintable_columns)
		};
		buffer.push_str(&value_str);
		buffer.push('\n');
	}

	ctx.writer
		.write_all(buffer.as_bytes())
		.await
		.into_diagnostic()?;
	ctx.writer.flush().await.into_diagnostic()?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	async fn run(query: &str) -> String {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");
		let pool = crate::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");
		let client = pool.get().await.expect("Failed to get connection");
		let rows = client.query(query, &[]).await.expect("Query failed");
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
		String::from_utf8(buffer).expect("Invalid UTF-8")
	}

	#[tokio::test]
	async fn test_plain_single_column() {
		let output = run("SELECT * FROM (VALUES ('a'), ('b'), ('c')) AS t(v)").await;
		assert_eq!(output, "a\nb\nc\n");
	}

	#[tokio::test]
	async fn test_plain_first_column_only() {
		let output = run("SELECT * FROM (VALUES (1, 'x'), (2, 'y')) AS t(id, name)").await;
		assert_eq!(output, "1\n2\n");
	}
}
