use std::ops::ControlFlow;

use comfy_table::Table;

use crate::repl::state::ReplContext;

pub(super) async fn handle_describe_view(
	ctx: &mut ReplContext<'_>,
	schema: &str,
	view_name: &str,
	detail: bool,
	sameconn: bool,
) -> ControlFlow<()> {
	let columns_query = r#"
		SELECT
			a.attname AS column_name,
			pg_catalog.format_type(a.atttypid, a.atttypmod) AS data_type,
			pg_catalog.col_description(c.oid, a.attnum) AS description
		FROM pg_catalog.pg_class c
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
		LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid
		WHERE n.nspname = $1
			AND c.relname = $2
			AND c.relkind IN ('v', 'm')
			AND a.attnum > 0
			AND NOT a.attisdropped
		ORDER BY a.attnum
	"#;

	let view_info_query = r#"
		SELECT
			c.relkind AS view_kind,
			pg_catalog.pg_get_viewdef(c.oid, true) AS view_definition,
			pg_catalog.pg_get_userbyid(c.relowner) AS owner,
			pg_size_pretty(pg_total_relation_size(c.oid)) AS size,
			obj_description(c.oid, 'pg_class') AS view_comment
		FROM pg_catalog.pg_class c
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
		WHERE n.nspname = $1
			AND c.relname = $2
			AND c.relkind IN ('v', 'm')
	"#;

	let columns_result = if sameconn {
		ctx.client
			.query(columns_query, &[&schema, &view_name])
			.await
	} else {
		match ctx.pool.get().await {
			Ok(client) => client.query(columns_query, &[&schema, &view_name]).await,
			Err(e) => {
				eprintln!("Error getting connection from pool: {}", e);
				return ControlFlow::Continue(());
			}
		}
	};

	match columns_result {
		Ok(rows) => {
			if rows.is_empty() {
				eprintln!("No columns found for view \"{}.{}\".", schema, view_name);
				return ControlFlow::Continue(());
			}

			let view_info_result = if sameconn {
				ctx.client
					.query(view_info_query, &[&schema, &view_name])
					.await
			} else {
				match ctx.pool.get().await {
					Ok(client) => client.query(view_info_query, &[&schema, &view_name]).await,
					Err(_) => {
						return ControlFlow::Continue(());
					}
				}
			};

			let (view_kind, view_definition, owner, size, view_comment) =
				if let Ok(info_rows) = view_info_result {
					if let Some(row) = info_rows.first() {
						let kind: String = row.get(0);
						let def: String = row.get(1);
						let own: String = row.get(2);
						let sz: String = row.get(3);
						let cmt: Option<String> = row.get(4);
						(Some(kind), Some(def), Some(own), Some(sz), cmt)
					} else {
						(None, None, None, None, None)
					}
				} else {
					(None, None, None, None, None)
				};

			let view_type = match view_kind.as_deref() {
				Some("m") => "Materialized View",
				Some("v") => "View",
				_ => "View",
			};

			println!("{} \"{}.{}\"", view_type, schema, view_name);

			let mut table = Table::new();
			crate::table::configure(&mut table);

			if detail {
				table.set_header(vec!["Column", "Type", "Description"]);
			} else {
				table.set_header(vec!["Column", "Type"]);
			}

			for row in rows {
				let column_name: String = row.get(0);
				let data_type: String = row.get(1);
				let description: Option<String> = if detail { row.get(2) } else { None };

				if detail {
					table.add_row(vec![
						column_name,
						data_type,
						description.unwrap_or_default(),
					]);
				} else {
					table.add_row(vec![column_name, data_type]);
				}
			}

			crate::table::style_header(&mut table);
			println!("{table}");

			if let Some(definition) = view_definition {
				println!("\nView definition:");
				println!("{}", definition);
			}

			if detail {
				if let Some(own) = owner {
					println!("\nOwner: {}", own);
				}
				if let Some(sz) = size {
					println!("Size: {}", sz);
				}
				if let Some(comment) = view_comment {
					if !comment.is_empty() {
						println!("Comment: {}", comment);
					}
				}
			}

			println!();
			ControlFlow::Continue(())
		}
		Err(e) => {
			eprintln!("Error describing view: {}", e);
			ControlFlow::Continue(())
		}
	}
}
