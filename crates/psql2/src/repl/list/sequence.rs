use std::ops::ControlFlow;

use comfy_table::Table;

use super::pattern::{parse_pattern, should_exclude_system_schemas};
use crate::repl::state::ReplContext;

pub(super) async fn handle_list_sequences(
	ctx: &mut ReplContext<'_>,
	pattern: &str,
	detail: bool,
	sameconn: bool,
) -> ControlFlow<()> {
	let (schema_pattern, sequence_pattern) = parse_pattern(pattern);
	let exclude_schemas = should_exclude_system_schemas(pattern);

	let query = if detail {
		if exclude_schemas {
			r#"
			SELECT
				n.nspname AS "Schema",
				c.relname AS "Name",
				pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size",
				pg_catalog.pg_get_userbyid(c.relowner) AS "Owner",
				CASE c.relpersistence
					WHEN 'p' THEN 'permanent'
					WHEN 'u' THEN 'unlogged'
					WHEN 't' THEN 'temporary'
				END AS "Persistence",
				format_type(s.seqtypid, NULL) AS "Type"
			FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			LEFT JOIN pg_catalog.pg_sequence s ON s.seqrelid = c.oid
			WHERE c.relkind = 'S'
				AND n.nspname ~ $1
				AND c.relname ~ $2
				AND n.nspname NOT IN ('information_schema', 'pg_toast')
			ORDER BY 1, 2
			"#
		} else {
			r#"
			SELECT
				n.nspname AS "Schema",
				c.relname AS "Name",
				pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size",
				pg_catalog.pg_get_userbyid(c.relowner) AS "Owner",
				CASE c.relpersistence
					WHEN 'p' THEN 'permanent'
					WHEN 'u' THEN 'unlogged'
					WHEN 't' THEN 'temporary'
				END AS "Persistence",
				format_type(s.seqtypid, NULL) AS "Type"
			FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			LEFT JOIN pg_catalog.pg_sequence s ON s.seqrelid = c.oid
			WHERE c.relkind = 'S'
				AND n.nspname ~ $1
				AND c.relname ~ $2
			ORDER BY 1, 2
			"#
		}
	} else if exclude_schemas {
		r#"
		SELECT
			n.nspname AS "Schema",
			c.relname AS "Name",
			pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size"
		FROM pg_catalog.pg_class c
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
		WHERE c.relkind = 'S'
			AND n.nspname ~ $1
			AND c.relname ~ $2
			AND n.nspname NOT IN ('information_schema', 'pg_toast')
		ORDER BY 1, 2
		"#
	} else {
		r#"
		SELECT
			n.nspname AS "Schema",
			c.relname AS "Name",
			pg_size_pretty(pg_total_relation_size(c.oid)) AS "Size"
		FROM pg_catalog.pg_class c
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
		WHERE c.relkind = 'S'
			AND n.nspname ~ $1
			AND c.relname ~ $2
		ORDER BY 1, 2
		"#
	};

	let result = if sameconn {
		ctx.client
			.query(query, &[&schema_pattern, &sequence_pattern])
			.await
	} else {
		match ctx.pool.get().await {
			Ok(client) => {
				client
					.query(query, &[&schema_pattern, &sequence_pattern])
					.await
			}
			Err(e) => {
				eprintln!("Error getting connection from pool: {}", e);
				return ControlFlow::Continue(());
			}
		}
	};

	match result {
		Ok(rows) => {
			if rows.is_empty() {
				println!("No matching sequences found.");
				return ControlFlow::Continue(());
			}

			let mut table = Table::new();
			crate::table::configure(&mut table);

			if detail {
				table.set_header(vec![
					"Schema",
					"Name",
					"Size",
					"Owner",
					"Persistence",
					"Type",
				]);
				for row in rows {
					let schema: String = row.get(0);
					let name: String = row.get(1);
					let size: String = row.get(2);
					let owner: String = row.get(3);
					let persistence: String = row.get(4);
					let seq_type: String = row.get(5);
					table.add_row(vec![schema, name, size, owner, persistence, seq_type]);
				}
			} else {
				table.set_header(vec!["Schema", "Name", "Size"]);
				for row in rows {
					let schema: String = row.get(0);
					let name: String = row.get(1);
					let size: String = row.get(2);
					table.add_row(vec![schema, name, size]);
				}
			}

			crate::table::style_header(&mut table);
			println!("{table}\n");
			ControlFlow::Continue(())
		}
		Err(e) => {
			eprintln!("Error listing sequences: {}", e);
			ControlFlow::Continue(())
		}
	}
}
