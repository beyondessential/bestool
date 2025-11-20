use std::ops::ControlFlow;

use bestool_postgres::error::format_error;
use comfy_table::Table;

use crate::repl::state::ReplContext;

use super::output::OutputWriter;

pub(super) async fn handle_describe_function(
	ctx: &mut ReplContext<'_>,
	schema: &str,
	function_name: &str,
	detail: bool,
	sameconn: bool,
	writer: &OutputWriter,
) -> ControlFlow<()> {
	let function_query = r#"
		SELECT
			p.proname AS function_name,
			n.nspname AS schema_name,
			pg_catalog.pg_get_function_result(p.oid) AS result_type,
			CASE p.provolatile
				WHEN 'i' THEN 'immutable'
				WHEN 's' THEN 'stable'
				WHEN 'v' THEN 'volatile'
			END AS volatility,
			CASE p.proparallel
				WHEN 's' THEN 'safe'
				WHEN 'r' THEN 'restricted'
				WHEN 'u' THEN 'unsafe'
			END AS parallel,
			l.lanname AS language,
			CASE
				WHEN p.prosecdef THEN 'definer'
				ELSE 'invoker'
			END AS security,
			obj_description(p.oid, 'pg_proc') AS description,
			pg_catalog.pg_get_functiondef(p.oid) AS function_definition
		FROM pg_catalog.pg_proc p
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
		LEFT JOIN pg_catalog.pg_language l ON l.oid = p.prolang
		WHERE n.nspname = $1
			AND p.proname = $2
	"#;

	let arguments_query = r#"
		SELECT
			COALESCE(p.proargnames[i], '') AS arg_name,
			pg_catalog.format_type(p.proargtypes[i-1], NULL) AS arg_type
		FROM pg_catalog.pg_proc p
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
		CROSS JOIN generate_series(1, p.pronargs) AS i
		WHERE n.nspname = $1
			AND p.proname = $2
		ORDER BY i
	"#;

	let result = if sameconn {
		ctx.client
			.query(function_query, &[&schema, &function_name])
			.await
	} else {
		match ctx.pool.get().await {
			Ok(client) => {
				client
					.query(function_query, &[&schema, &function_name])
					.await
			}
			Err(e) => {
				eprintln!("Error getting connection from pool: {}", format_error(&e));
				return ControlFlow::Continue(());
			}
		}
	};

	match result {
		Ok(rows) => {
			if rows.is_empty() {
				eprintln!(
					"Did not find any function named \"{}.{}\".",
					schema, function_name
				);
				return ControlFlow::Continue(());
			}

			let row = &rows[0];
			let function_name_val: String = row.get(0);
			let schema_name: String = row.get(1);
			let result_type: String = row.get(2);
			let volatility: String = row.get(3);
			let parallel: String = row.get(4);
			let language: String = row.get(5);
			let security: String = row.get(6);
			let description: Option<String> = row.get(7);
			let function_definition: String = row.get(8);

			writer
				.writeln(&format!(
					"Function \"{}.{}\"",
					schema_name, function_name_val
				))
				.await;

			let mut properties = Vec::new();
			properties.push(language.as_str());
			properties.push(volatility.as_str());
			let parallel_str = format!("parallel {}", parallel);
			properties.push(parallel_str.as_str());
			if security == "definer" {
				properties.push("security definer");
			}
			writer
				.writeln(&format!("    {}", properties.join(", ")))
				.await;

			writer.writeln(&format!("Returns: {}", result_type)).await;

			let args_result = if sameconn {
				ctx.client
					.query(arguments_query, &[&schema, &function_name])
					.await
			} else {
				match ctx.pool.get().await {
					Ok(client) => {
						client
							.query(arguments_query, &[&schema, &function_name])
							.await
					}
					Err(e) => {
						eprintln!("Error getting connection from pool: {}", format_error(&e));
						return ControlFlow::Continue(());
					}
				}
			};

			if let Ok(arg_rows) = args_result
				&& !arg_rows.is_empty()
			{
				println!();
				let mut table = Table::new();
				crate::table::configure(&mut table);

				table.set_header(vec!["Argument name", "Type"]);
				for arg_row in arg_rows {
					let arg_name: String = arg_row.get(0);
					let arg_type: String = arg_row.get(1);
					let name_display = if arg_name.is_empty() {
						"(unnamed)".to_string()
					} else {
						arg_name
					};
					table.add_row(vec![name_display, arg_type]);
				}

				crate::table::style_header(&mut table);
				writer.writeln(&format!("{table}")).await;
			}

			if detail {
				if let Some(desc) = description
					&& !desc.is_empty()
				{
					writer.writeln(&format!("\nDescription: {}", desc)).await;
				}

				writer
					.writeln(&format!("\nDefinition:\n{}", function_definition))
					.await;
			}

			println!();
			ControlFlow::Continue(())
		}
		Err(e) => {
			eprintln!(
				"Error describing function \"{}.{}\": {}",
				schema,
				function_name,
				format_error(&e)
			);
			ControlFlow::Continue(())
		}
	}
}
