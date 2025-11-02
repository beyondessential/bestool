use miette::{IntoDiagnostic, Result};
use rusqlite::Connection;
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

	// Create or open SQLite database
	let mut conn = Connection::open(Path::new(file_path)).into_diagnostic()?;

	// Build CREATE TABLE statement
	let mut create_sql = String::from("CREATE TABLE IF NOT EXISTS results (");
	for (idx, &i) in column_indices.iter().enumerate() {
		if idx > 0 {
			create_sql.push_str(", ");
		}
		let column_name = ctx.columns[i].name();
		// SQLite will store any type in TEXT columns
		create_sql.push_str(&format!("\"{}\" TEXT", column_name));
	}
	create_sql.push(')');

	conn.execute(&create_sql, []).into_diagnostic()?;

	// Build INSERT statement
	let mut insert_sql = String::from("INSERT INTO results (");
	for (idx, &i) in column_indices.iter().enumerate() {
		if idx > 0 {
			insert_sql.push_str(", ");
		}
		insert_sql.push_str(&format!("\"{}\"", ctx.columns[i].name()));
	}
	insert_sql.push_str(") VALUES (");
	for idx in 0..column_indices.len() {
		if idx > 0 {
			insert_sql.push_str(", ");
		}
		insert_sql.push('?');
	}
	insert_sql.push(')');

	// Insert data rows
	let tx = conn.transaction().into_diagnostic()?;
	{
		let mut stmt = tx.prepare(&insert_sql).into_diagnostic()?;

		for (row_idx, row) in ctx.rows.iter().enumerate() {
			let values: Vec<Option<String>> = column_indices
				.iter()
				.map(|&i| {
					let value_str =
						column::get_value(row, i, row_idx, ctx.unprintable_columns, ctx.text_rows);
					if value_str == "NULL" {
						None
					} else {
						Some(value_str)
					}
				})
				.collect();

			// Convert Vec<Option<String>> to Vec<&dyn ToSql>
			let params: Vec<&dyn rusqlite::ToSql> =
				values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

			stmt.execute(params.as_slice()).into_diagnostic()?;
		}
	}
	tx.commit().into_diagnostic()?;

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_sqlite_display() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let rows = client
			.query(
				"SELECT 1 as id, 'Alice' as name, 25 as age, NULL as notes UNION ALL SELECT 2, 'Bob', 30, 'test note'",
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

		// Verify the SQLite database was created and has the correct data
		let verify_conn = rusqlite::Connection::open(&file_path).unwrap();
		let mut stmt = verify_conn
			.prepare("SELECT id, name, age, notes FROM results ORDER BY id")
			.unwrap();
		let mut result_rows = stmt.query([]).unwrap();

		let row1 = result_rows.next().unwrap().unwrap();
		assert_eq!(row1.get::<_, String>(0).unwrap(), "1");
		assert_eq!(row1.get::<_, String>(1).unwrap(), "Alice");
		assert_eq!(row1.get::<_, String>(2).unwrap(), "25");
		assert!(row1.get::<_, Option<String>>(3).unwrap().is_none());

		let row2 = result_rows.next().unwrap().unwrap();
		assert_eq!(row2.get::<_, String>(0).unwrap(), "2");
		assert_eq!(row2.get::<_, String>(1).unwrap(), "Bob");
		assert_eq!(row2.get::<_, String>(2).unwrap(), "30");
		assert_eq!(row2.get::<_, String>(3).unwrap(), "test note");

		assert!(result_rows.next().unwrap().is_none());
	}
}
