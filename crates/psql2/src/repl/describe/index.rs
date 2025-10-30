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
			obj_description(i.oid, 'pg_class') AS description
		FROM pg_catalog.pg_class i
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = i.relnamespace
		LEFT JOIN pg_catalog.pg_index ix ON ix.indexrelid = i.oid
		LEFT JOIN pg_catalog.pg_class t ON t.oid = ix.indrelid
		LEFT JOIN pg_catalog.pg_am am ON i.relam = am.oid
		WHERE n.nspname = $1
			AND i.relname = $2
			AND i.relkind IN ('i', 'I')
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

			println!("Index \"{}.{}\"", schema_name, index_name_val);
			println!();

			let mut table = Table::new();
			crate::table::configure(&mut table);

			table.set_header(vec!["Property", "Value"]);
			table.add_row(vec!["Table", &format!("{}.{}", schema_name, table_name)]);
			table.add_row(vec!["Type", &index_type]);
			table.add_row(vec!["Unique", &is_unique]);
			table.add_row(vec!["Primary", &is_primary]);
			table.add_row(vec!["Valid", &is_valid]);

			if detail {
				table.add_row(vec!["Size", &size]);
				table.add_row(vec!["Owner", &owner]);
			}

			crate::table::style_header(&mut table);
			println!("{table}");

			println!("\nDefinition:");
			println!("    {}", index_definition);

			if detail {
				if let Some(desc) = description {
					if !desc.is_empty() {
						println!("\nDescription:");
						println!("    {}", desc);
					}
				}
			}

			println!();
			ControlFlow::Continue(())
		}
		Err(e) => {
			eprintln!("Error describing index: {}", e);
			ControlFlow::Continue(())
		}
	}
}
