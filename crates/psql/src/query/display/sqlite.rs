use std::{path::Path, sync::Arc};

use miette::{IntoDiagnostic, Result};
use turso_core::{CheckpointMode, PlatformIO};

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

	if Path::new(file_path).exists() {
		return Err(miette::miette!("File already exists"));
	}

	// Create SQLite database the normal way
	{
		let db = turso::Builder::new_local(file_path)
			.build()
			.await
			.into_diagnostic()?;
		let mut conn = db.connect().into_diagnostic()?;

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

		conn.execute(&create_sql, ()).await.into_diagnostic()?;

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
		let tx = conn.transaction().await.into_diagnostic()?;
		{
			for (row_idx, row) in ctx.rows.iter().enumerate() {
				let values: Vec<turso::Value> = column_indices
					.iter()
					.map(|&i| {
						let value_str = column::get_value(
							row,
							i,
							row_idx,
							ctx.unprintable_columns,
							ctx.text_rows,
						);
						if value_str == "NULL" {
							turso::Value::Null
						} else {
							turso::Value::Text(value_str)
						}
					})
					.collect();

				tx.execute(&insert_sql, turso::params_from_iter(values))
					.await
					.into_diagnostic()?;
			}
		}
		tx.commit().await.into_diagnostic()?;
	}

	// Forcefully checkpoint the database
	// This has to use the core API because this is not exposed through the public API
	// See also <https://github.com/tursodatabase/turso/issues/1906>
	{
		let io = Arc::new(PlatformIO::new().into_diagnostic()?);
		let db = turso_core::Database::open_file(io, file_path, false, false).into_diagnostic()?;
		let conn = db.connect().into_diagnostic()?;
		conn.checkpoint(CheckpointMode::Truncate {
			upper_bound_inclusive: None,
		})
		.into_diagnostic()?;
		conn.close().into_diagnostic()?;
	}

	// Remove the WAL file, which is now empty since we just checkpointed
	std::fs::remove_file(format!("{file_path}-wal")).into_diagnostic()?;

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
		let verify_db = turso::Builder::new_local(&file_path).build().await.unwrap();
		let verify_conn = verify_db.connect().unwrap();
		let mut result_rows = verify_conn
			.query("SELECT id, name, age, notes FROM results ORDER BY id", ())
			.await
			.unwrap();

		let row1 = result_rows.next().await.unwrap().unwrap();
		assert_eq!(row1.get_value(0).unwrap().as_text().unwrap(), "1");
		assert_eq!(row1.get_value(1).unwrap().as_text().unwrap(), "Alice");
		assert_eq!(row1.get_value(2).unwrap().as_text().unwrap(), "25");
		assert!(matches!(row1.get_value(3).unwrap(), turso::Value::Null));

		let row2 = result_rows.next().await.unwrap().unwrap();
		assert_eq!(row2.get_value(0).unwrap().as_text().unwrap(), "2");
		assert_eq!(row2.get_value(1).unwrap().as_text().unwrap(), "Bob");
		assert_eq!(row2.get_value(2).unwrap().as_text().unwrap(), "30");
		assert_eq!(row2.get_value(3).unwrap().as_text().unwrap(), "test note");

		assert!(result_rows.next().await.unwrap().is_none());
	}
}
