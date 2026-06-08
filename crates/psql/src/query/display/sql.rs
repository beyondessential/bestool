use bestool_postgres::{
	stringify::{get_value, is_null, sql_quote},
	text_cast::CellRef,
};
use miette::{IntoDiagnostic, Result};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::colors::REDACTED_VALUE;

/// Whether an identifier must be quoted to be used bare as a column or table name.
///
/// Only reserved and type/function-name keywords are disallowed in a name position;
/// unreserved and column-name keywords (e.g. `name`, `type`) are fine unquoted.
fn must_quote_keyword(name: &str) -> bool {
	use pg_query::protobuf::KeywordKind;
	match pg_query::scan(name) {
		Ok(result) => result.tokens.iter().any(|t| {
			matches!(
				t.keyword_kind(),
				KeywordKind::ReservedKeyword | KeywordKind::TypeFuncNameKeyword
			)
		}),
		// If it won't scan cleanly, quote to be safe.
		Err(_) => true,
	}
}

/// Quote a single SQL identifier, leaving simple lowercase non-keyword identifiers bare.
fn quote_ident(name: &str) -> String {
	let mut chars = name.chars();
	let syntactic_ok = match chars.next() {
		Some(c) if c.is_ascii_lowercase() || c == '_' => {
			chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
		}
		_ => false,
	};
	if syntactic_ok && !must_quote_keyword(name) {
		name.to_string()
	} else {
		format!("\"{}\"", name.replace('"', "\"\""))
	}
}

/// Quote a possibly schema-qualified name (`schema.table`), quoting each part as needed.
fn quote_qualified(name: &str) -> String {
	name.split('.')
		.map(quote_ident)
		.collect::<Vec<_>>()
		.join(".")
}

/// SQL output: render rows as INSERT statements into `table`. When `expanded` is true,
/// emit one INSERT per row; otherwise a single INSERT with one VALUES tuple per row.
pub async fn display<W: AsyncWrite + Unpin>(
	ctx: &mut super::DisplayContext<'_, W>,
	expanded: bool,
	table: &str,
) -> Result<()> {
	let column_indices: Vec<usize> = if let Some(indices) = ctx.column_indices {
		indices.to_vec()
	} else {
		(0..ctx.columns.len()).collect()
	};

	if column_indices.is_empty() || ctx.rows.is_empty() {
		return Ok(());
	}

	// Batch-cast all unprintable, non-redacted cells (mirrors csv.rs).
	let mut unprintable_cells = Vec::new();
	for row_idx in 0..ctx.rows.len() {
		for &col_idx in &column_indices {
			if ctx.unprintable_columns.contains(&col_idx) && !ctx.should_redact(col_idx) {
				unprintable_cells.push(CellRef { row_idx, col_idx });
			}
		}
	}
	let mut cast_map = std::collections::HashMap::new();
	if !unprintable_cells.is_empty()
		&& let Some(text_caster) = &ctx.text_caster
	{
		let results = text_caster.cast_batch(ctx.rows, &unprintable_cells).await;
		for (cell, result) in unprintable_cells.iter().zip(results) {
			cast_map.insert(*cell, result);
		}
	}

	let table = quote_qualified(table);
	let columns: Vec<String> = column_indices
		.iter()
		.map(|&i| quote_ident(ctx.columns[i].name()))
		.collect();
	let column_list = columns.join(", ");

	let mut value_tuples: Vec<String> = Vec::with_capacity(ctx.rows.len());
	for (row_idx, row) in ctx.rows.iter().enumerate() {
		let mut values = Vec::with_capacity(column_indices.len());
		for &col_idx in &column_indices {
			let ty = ctx.columns[col_idx].type_();
			let literal = if ctx.should_redact(col_idx) {
				sql_quote(ty, REDACTED_VALUE, false)
			} else if is_null(row, col_idx) {
				"NULL".to_string()
			} else if ctx.unprintable_columns.contains(&col_idx) {
				let text = match cast_map.get(&CellRef { row_idx, col_idx }) {
					Some(Ok(text)) => text.clone(),
					Some(Err(_)) => "(error)".to_string(),
					None => "(binary data)".to_string(),
				};
				sql_quote(ty, &text, false)
			} else {
				let text = get_value(row, col_idx, ctx.unprintable_columns);
				sql_quote(ty, &text, false)
			};
			values.push(literal);
		}
		value_tuples.push(format!("({})", values.join(", ")));
	}

	let mut buffer = String::new();
	if expanded {
		for tuple in &value_tuples {
			buffer.push_str(&format!(
				"INSERT INTO {table} ({column_list}) VALUES {tuple};\n"
			));
		}
	} else {
		buffer.push_str(&format!("INSERT INTO {table} ({column_list}) VALUES\n"));
		for (i, tuple) in value_tuples.iter().enumerate() {
			let sep = if i + 1 == value_tuples.len() {
				";"
			} else {
				","
			};
			buffer.push_str(&format!("\t{tuple}{sep}\n"));
		}
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

	async fn run(query: &str, expanded: bool, table: &str) -> String {
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
		display(&mut ctx, expanded, table)
			.await
			.expect("Display failed");
		String::from_utf8(buffer).expect("Invalid UTF-8")
	}

	#[tokio::test]
	async fn test_sql_multi_row() {
		let output = run(
			"SELECT * FROM (VALUES (1, 'Alice'), (2, 'Bob')) AS t(id, name)",
			false,
			"patients",
		)
		.await;
		assert_eq!(
			output,
			"INSERT INTO patients (id, name) VALUES\n\t(1, 'Alice'),\n\t(2, 'Bob');\n"
		);
	}

	#[tokio::test]
	async fn test_sql_expanded() {
		let output = run(
			"SELECT * FROM (VALUES (1, 'Alice'), (2, 'Bob')) AS t(id, name)",
			true,
			"patients",
		)
		.await;
		assert_eq!(
			output,
			"INSERT INTO patients (id, name) VALUES (1, 'Alice');\nINSERT INTO patients (id, name) VALUES (2, 'Bob');\n"
		);
	}

	#[tokio::test]
	async fn test_sql_quoting_and_nulls() {
		let output = run(
			"SELECT 'O''Brien'::text AS name, 42::int AS n, true::bool AS active, NULL::text AS note",
			false,
			"t",
		)
		.await;
		assert_eq!(
			output,
			"INSERT INTO t (name, n, active, note) VALUES\n\t('O''Brien', 42, TRUE, NULL);\n"
		);
	}

	#[tokio::test]
	async fn test_sql_quotes_keyword_identifier() {
		let output = run(
			"SELECT 1 AS \"order\", 2 AS normal",
			false,
			"public.my_table",
		)
		.await;
		assert_eq!(
			output,
			"INSERT INTO public.my_table (\"order\", normal) VALUES\n\t(1, 2);\n"
		);
	}
}
