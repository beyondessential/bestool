use std::ops::ControlFlow;

use comfy_table::Table;

use super::pattern::{parse_pattern, should_exclude_system_schemas};
use crate::repl::state::ReplContext;

pub(super) async fn handle_list_schemas(
	ctx: &mut ReplContext<'_>,
	pattern: &str,
	detail: bool,
	sameconn: bool,
) -> ControlFlow<()> {
	let (schema_pattern, _) = parse_pattern(pattern);
	let exclude_schemas = should_exclude_system_schemas(pattern);

	let query = if detail {
		if exclude_schemas {
			r#"
			SELECT
				n.nspname AS "Name",
				pg_catalog.pg_get_userbyid(n.nspowner) AS "Owner",
				pg_catalog.array_to_string(n.nspacl, E'\n') AS "ACL"
			FROM pg_catalog.pg_namespace n
			WHERE n.nspname ~ $1
				AND n.nspname NOT LIKE 'pg_%'
				AND n.nspname NOT IN ('information_schema')
			ORDER BY 1
			"#
		} else {
			r#"
			SELECT
				n.nspname AS "Name",
				pg_catalog.pg_get_userbyid(n.nspowner) AS "Owner",
				pg_catalog.array_to_string(n.nspacl, E'\n') AS "ACL"
			FROM pg_catalog.pg_namespace n
			WHERE n.nspname ~ $1
			ORDER BY 1
			"#
		}
	} else if exclude_schemas {
		r#"
		SELECT
			n.nspname AS "Name",
			pg_catalog.pg_get_userbyid(n.nspowner) AS "Owner"
		FROM pg_catalog.pg_namespace n
		WHERE n.nspname ~ $1
			AND n.nspname NOT LIKE 'pg_%'
			AND n.nspname NOT IN ('information_schema')
		ORDER BY 1
		"#
	} else {
		r#"
		SELECT
			n.nspname AS "Name",
			pg_catalog.pg_get_userbyid(n.nspowner) AS "Owner"
		FROM pg_catalog.pg_namespace n
		WHERE n.nspname ~ $1
		ORDER BY 1
		"#
	};

	let result = if sameconn {
		// Use the existing connection
		ctx.client.query(query, &[&schema_pattern]).await
	} else {
		// Get a new connection from the pool
		match ctx.pool.get().await {
			Ok(client) => client.query(query, &[&schema_pattern]).await,
			Err(e) => {
				eprintln!("Error getting connection from pool: {}", e);
				return ControlFlow::Continue(());
			}
		}
	};

	match result {
		Ok(rows) => {
			if rows.is_empty() {
				println!("No matching schemas found.");
				return ControlFlow::Continue(());
			}

			let mut table = Table::new();
			crate::table::configure(&mut table);

			if detail {
				table.set_header(vec!["Name", "Owner", "ACL"]);
				for row in rows {
					let name: String = row.get(0);
					let owner: String = row.get(1);
					let acl: Option<String> = row.get(2);
					table.add_row(vec![name, owner, acl.unwrap_or_default()]);
				}
			} else {
				table.set_header(vec!["Name", "Owner"]);
				for row in rows {
					let name: String = row.get(0);
					let owner: String = row.get(1);
					table.add_row(vec![name, owner]);
				}
			}

			crate::table::style_header(&mut table);
			println!("{table}\n");
			ControlFlow::Continue(())
		}
		Err(e) => {
			eprintln!("Error listing schemas: {}", e);
			ControlFlow::Continue(())
		}
	}
}
