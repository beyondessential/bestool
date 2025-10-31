use std::ops::ControlFlow;

use comfy_table::Table;

use super::pattern::{parse_pattern, should_exclude_system_schemas};
use crate::repl::state::ReplContext;

pub(super) async fn handle_list_functions(
	ctx: &mut ReplContext<'_>,
	pattern: &str,
	detail: bool,
	sameconn: bool,
) -> ControlFlow<()> {
	let (schema_pattern, function_pattern) = parse_pattern(pattern);
	let exclude_schemas = should_exclude_system_schemas(pattern);

	let query = if detail {
		if exclude_schemas {
			r#"
			SELECT
				n.nspname AS "Schema",
				p.proname AS "Name",
				pg_catalog.pg_get_function_result(p.oid) AS "Result data type",
				pg_catalog.pg_get_function_arguments(p.oid) AS "Argument data types",
				CASE p.prokind
					WHEN 'f' THEN 'function'
					WHEN 'p' THEN 'procedure'
					WHEN 'a' THEN 'aggregate'
					WHEN 'w' THEN 'window'
				END AS "Type",
				CASE
					WHEN p.provolatile = 'i' THEN 'immutable'
					WHEN p.provolatile = 's' THEN 'stable'
					WHEN p.provolatile = 'v' THEN 'volatile'
				END AS "Volatility",
				pg_catalog.pg_get_userbyid(p.proowner) AS "Owner",
				CASE
					WHEN prosecdef THEN 'definer'
					ELSE 'invoker'
				END AS "Security",
				CASE
					WHEN p.proacl IS NULL THEN NULL
					ELSE pg_catalog.array_to_string(p.proacl, E'\n')
				END AS "ACL",
				l.lanname AS "Language",
				pg_catalog.obj_description(p.oid, 'pg_proc') AS "Description"
			FROM pg_catalog.pg_proc p
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
			LEFT JOIN pg_catalog.pg_language l ON l.oid = p.prolang
			WHERE n.nspname ~ $1
				AND p.proname ~ $2
				AND n.nspname NOT IN ('information_schema', 'pg_toast')
				AND n.nspname NOT LIKE 'pg_%'
			ORDER BY 1, 2
			"#
		} else {
			r#"
			SELECT
				n.nspname AS "Schema",
				p.proname AS "Name",
				pg_catalog.pg_get_function_result(p.oid) AS "Result data type",
				pg_catalog.pg_get_function_arguments(p.oid) AS "Argument data types",
				CASE p.prokind
					WHEN 'f' THEN 'function'
					WHEN 'p' THEN 'procedure'
					WHEN 'a' THEN 'aggregate'
					WHEN 'w' THEN 'window'
				END AS "Type",
				CASE
					WHEN p.provolatile = 'i' THEN 'immutable'
					WHEN p.provolatile = 's' THEN 'stable'
					WHEN p.provolatile = 'v' THEN 'volatile'
				END AS "Volatility",
				pg_catalog.pg_get_userbyid(p.proowner) AS "Owner",
				CASE
					WHEN prosecdef THEN 'definer'
					ELSE 'invoker'
				END AS "Security",
				CASE
					WHEN p.proacl IS NULL THEN NULL
					ELSE pg_catalog.array_to_string(p.proacl, E'\n')
				END AS "ACL",
				l.lanname AS "Language",
				pg_catalog.obj_description(p.oid, 'pg_proc') AS "Description"
			FROM pg_catalog.pg_proc p
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
			LEFT JOIN pg_catalog.pg_language l ON l.oid = p.prolang
			WHERE n.nspname ~ $1
				AND p.proname ~ $2
			ORDER BY 1, 2
			"#
		}
	} else if exclude_schemas {
		r#"
		SELECT
			n.nspname AS "Schema",
			p.proname AS "Name",
			pg_catalog.pg_get_function_result(p.oid) AS "Result data type",
			pg_catalog.pg_get_function_arguments(p.oid) AS "Argument data types",
			CASE p.prokind
				WHEN 'f' THEN 'function'
				WHEN 'p' THEN 'procedure'
				WHEN 'a' THEN 'aggregate'
				WHEN 'w' THEN 'window'
			END AS "Type"
		FROM pg_catalog.pg_proc p
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
		WHERE n.nspname ~ $1
			AND p.proname ~ $2
			AND n.nspname NOT IN ('information_schema', 'pg_toast')
			AND n.nspname NOT LIKE 'pg_%'
		ORDER BY 1, 2
		"#
	} else {
		r#"
		SELECT
			n.nspname AS "Schema",
			p.proname AS "Name",
			pg_catalog.pg_get_function_result(p.oid) AS "Result data type",
			pg_catalog.pg_get_function_arguments(p.oid) AS "Argument data types",
			CASE p.prokind
				WHEN 'f' THEN 'function'
				WHEN 'p' THEN 'procedure'
				WHEN 'a' THEN 'aggregate'
				WHEN 'w' THEN 'window'
			END AS "Type"
		FROM pg_catalog.pg_proc p
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
		WHERE n.nspname ~ $1
			AND p.proname ~ $2
		ORDER BY 1, 2
		"#
	};

	let result = if sameconn {
		// Use the existing connection
		ctx.client
			.query(query, &[&schema_pattern, &function_pattern])
			.await
	} else {
		// Get a new connection from the pool
		match ctx.pool.get().await {
			Ok(client) => {
				client
					.query(query, &[&schema_pattern, &function_pattern])
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
				println!("No matching functions found.");
				return ControlFlow::Continue(());
			}

			let mut table = Table::new();
			crate::table::configure(&mut table);

			if detail {
				table.set_header(vec![
					"Schema",
					"Name",
					"Result data type",
					"Argument data types",
					"Type",
					"Volatility",
					"Owner",
					"Security",
					"ACL",
					"Language",
					"Description",
				]);
				for row in rows {
					let schema: String = row.get(0);
					let name: String = row.get(1);
					let result_type: String = row.get(2);
					let args: String = row.get(3);
					let func_type: Option<String> = row.get(4);
					let volatility: Option<String> = row.get(5);
					let owner: String = row.get(6);
					let security: String = row.get(7);
					let acl: Option<String> = row.get(8);
					let language: Option<String> = row.get(9);
					let description: Option<String> = row.get(10);
					table.add_row(vec![
						schema,
						name,
						result_type,
						args,
						func_type.unwrap_or_default(),
						volatility.unwrap_or_default(),
						owner,
						security,
						acl.unwrap_or_default(),
						language.unwrap_or_default(),
						description.unwrap_or_default(),
					]);
				}
			} else {
				table.set_header(vec![
					"Schema",
					"Name",
					"Result data type",
					"Argument data types",
					"Type",
				]);
				for row in rows {
					let schema: String = row.get(0);
					let name: String = row.get(1);
					let result_type: String = row.get(2);
					let args: String = row.get(3);
					let func_type: Option<String> = row.get(4);
					table.add_row(vec![
						schema,
						name,
						result_type,
						args,
						func_type.unwrap_or_default(),
					]);
				}
			}

			crate::table::style_header(&mut table);
			println!("{table}\n");
			ControlFlow::Continue(())
		}
		Err(e) => {
			eprintln!("Error listing functions: {}", e);
			ControlFlow::Continue(())
		}
	}
}
