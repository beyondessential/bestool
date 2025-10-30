use std::ops::ControlFlow;

use comfy_table::Table;

use crate::repl::state::ReplContext;

pub(super) async fn handle_describe_index(
	ctx: &mut ReplContext<'_>,
	schema: &str,
	index_name: &str,
	detail: bool,
	sameconn: bool,
) -> ControlFlow<()> {
	let query = r#"
		SELECT
			i.relname AS index_name,
			t.relname AS table_name,
			n.nspname AS schema_name,
			am.amname AS index_type,
			pg_catalog.pg_get_indexdef(i.oid, 0, true) AS index_definition,
			CASE
				WHEN ix.indisprimary THEN 'yes'
				ELSE 'no'
			END AS is_primary,
			CASE
				WHEN ix.indisunique THEN 'yes'
				ELSE 'no'
			END AS is_unique,
			CASE
				WHEN ix.indisvalid THEN 'yes'
				ELSE 'no'
			END AS is_valid,
			pg_size_pretty(pg_total_relation_size(i.oid)) AS size,
			pg_catalog.pg_get_userbyid(i.relowner) AS owner,
			obj_description(i.oid, 'pg_class') AS description,
			ix.indkey AS index_keys
		FROM pg_catalog.pg_class i
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = i.relnamespace
		LEFT JOIN pg_catalog.pg_index ix ON ix.indexrelid = i.oid
		LEFT JOIN pg_catalog.pg_class t ON t.oid = ix.indrelid
		LEFT JOIN pg_catalog.pg_am am ON i.relam = am.oid
		WHERE n.nspname = $1
			AND i.relname = $2
			AND i.relkind IN ('i', 'I')
	"#;

	let columns_query = r#"
		SELECT
			a.attname AS column_name,
			pg_catalog.format_type(a.atttypid, a.atttypmod) AS data_type,
			CASE
				WHEN a.attnum = ANY(ix.indkey) THEN 'yes'
				ELSE 'no'
			END AS is_key,
			pg_catalog.pg_get_indexdef(i.oid, a.attnum - t.relnatts, true) AS definition,
			CASE
				WHEN a.attstorage = 'p' THEN 'plain'
				WHEN a.attstorage = 'e' THEN 'external'
				WHEN a.attstorage = 'm' THEN 'main'
				WHEN a.attstorage = 'x' THEN 'extended'
				ELSE ''
			END AS storage
		FROM pg_catalog.pg_class i
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = i.relnamespace
		LEFT JOIN pg_catalog.pg_index ix ON ix.indexrelid = i.oid
		LEFT JOIN pg_catalog.pg_class t ON t.oid = ix.indrelid
		LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid = t.oid
		WHERE n.nspname = $1
			AND i.relname = $2
			AND i.relkind IN ('i', 'I')
			AND a.attnum > 0
			AND NOT a.attisdropped
			AND a.attnum = ANY(ix.indkey)
		ORDER BY array_position(ix.indkey, a.attnum)
	"#;

	let result = if sameconn {
		ctx.client.query(query, &[&schema, &index_name]).await
	} else {
		match ctx.pool.get().await {
			Ok(client) => client.query(query, &[&schema, &index_name]).await,
			Err(e) => {
				eprintln!("Error getting connection from pool: {}", e);
				return ControlFlow::Continue(());
			}
		}
	};

	match result {
		Ok(rows) => {
			if rows.is_empty() {
				eprintln!(
					"Did not find any index named \"{}.{}\".",
					schema, index_name
				);
				return ControlFlow::Continue(());
			}

			let row = &rows[0];
			let index_name_val: String = row.get(0);
			let table_name: String = row.get(1);
			let schema_name: String = row.get(2);
			let index_type: String = row.get(3);
			let index_definition: String = row.get(4);
			let is_primary: String = row.get(5);
			let is_unique: String = row.get(6);
			let is_valid: String = row.get(7);
			let size: String = row.get(8);
			let owner: String = row.get(9);
			let description: Option<String> = row.get(10);

			println!(
				"Index \"{}.{}\" on table \"{}.{}\"",
				schema_name, index_name_val, schema_name, table_name
			);

			let mut properties = Vec::new();
			if is_unique == "yes" {
				properties.push("unique");
			}
			if is_primary == "yes" {
				properties.push("primary key");
			}
			properties.push(&index_type);
			if is_valid == "yes" {
				properties.push("valid");
			} else {
				properties.push("invalid");
			}
			println!("    {}", properties.join(", "));

			if detail {
				println!("Size: {}", size);
			}

			let columns_result = if sameconn {
				ctx.client
					.query(columns_query, &[&schema, &index_name])
					.await
			} else {
				match ctx.pool.get().await {
					Ok(client) => client.query(columns_query, &[&schema, &index_name]).await,
					Err(e) => {
						eprintln!("Error getting connection from pool: {}", e);
						return ControlFlow::Continue(());
					}
				}
			};

			if let Ok(col_rows) = columns_result {
				if !col_rows.is_empty() {
					println!();
					let mut table = Table::new();
					crate::table::configure(&mut table);

					if detail {
						table.set_header(vec!["Column", "Type", "Key?", "Definition", "Storage"]);
						for col_row in col_rows {
							let column_name: String = col_row.get(0);
							let data_type: String = col_row.get(1);
							let is_key: String = col_row.get(2);
							let definition: Option<String> = col_row.get(3);
							let storage: String = col_row.get(4);
							table.add_row(vec![
								column_name,
								data_type,
								is_key,
								definition.unwrap_or_default(),
								storage,
							]);
						}
					} else {
						table.set_header(vec!["Column", "Type", "Key?", "Definition"]);
						for col_row in col_rows {
							let column_name: String = col_row.get(0);
							let data_type: String = col_row.get(1);
							let is_key: String = col_row.get(2);
							let definition: Option<String> = col_row.get(3);
							table.add_row(vec![
								column_name,
								data_type,
								is_key,
								definition.unwrap_or_default(),
							]);
						}
					}

					crate::table::style_header(&mut table);
					println!("{table}");
				}
			}

			println!("\nDefinition:");
			println!("    {}", index_definition);

			if detail {
				println!("\nOwner: {}", owner);
				if let Some(desc) = description {
					if !desc.is_empty() {
						println!("Description: {}", desc);
					}
				}
			}

			println!();
			ControlFlow::Continue(())
		}
		Err(e) => {
			eprintln!(
				"Error describing index \"{}.{}\": {}",
				schema, index_name, e
			);
			ControlFlow::Continue(())
		}
	}
}
