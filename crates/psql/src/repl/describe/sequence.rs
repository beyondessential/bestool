use std::ops::ControlFlow;

use comfy_table::Table;

use crate::repl::state::ReplContext;

use super::output::OutputWriter;

pub(super) async fn handle_describe_sequence(
	ctx: &mut ReplContext<'_>,
	schema: &str,
	sequence_name: &str,
	detail: bool,
	sameconn: bool,
	writer: &OutputWriter,
) -> ControlFlow<()> {
	let sequence_query = r#"
		SELECT
			seqstart AS start_value,
			seqmin AS min_value,
			seqmax AS max_value,
			seqincrement AS increment_by,
			seqcycle AS is_cycle,
			seqcache AS cache_size
		FROM pg_catalog.pg_sequence s
		LEFT JOIN pg_catalog.pg_class c ON c.oid = s.seqrelid
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
		WHERE n.nspname = $1
			AND c.relname = $2
	"#;

	let sequence_info_query = r#"
		SELECT
			pg_catalog.pg_get_userbyid(c.relowner) AS owner,
			obj_description(c.oid, 'pg_class') AS description,
			format_type(s.seqtypid, NULL) AS data_type
		FROM pg_catalog.pg_class c
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
		LEFT JOIN pg_catalog.pg_sequence s ON s.seqrelid = c.oid
		WHERE n.nspname = $1
			AND c.relname = $2
			AND c.relkind = 'S'
	"#;

	let owned_by_query = r#"
		SELECT
			dependent_ns.nspname || '.' || dependent_table.relname || '.' || dependent_attr.attname AS owned_by
		FROM pg_catalog.pg_depend d
		JOIN pg_catalog.pg_class c ON c.oid = d.objid
		JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
		JOIN pg_catalog.pg_class dependent_table ON dependent_table.oid = d.refobjid
		JOIN pg_catalog.pg_namespace dependent_ns ON dependent_ns.oid = dependent_table.relnamespace
		JOIN pg_catalog.pg_attribute dependent_attr ON dependent_attr.attrelid = d.refobjid AND dependent_attr.attnum = d.refobjsubid
		WHERE n.nspname = $1
			AND c.relname = $2
			AND c.relkind = 'S'
			AND d.deptype = 'a'
	"#;

	let result = if sameconn {
		ctx.client
			.query(sequence_query, &[&schema, &sequence_name])
			.await
	} else {
		match ctx.pool.get().await {
			Ok(client) => {
				client
					.query(sequence_query, &[&schema, &sequence_name])
					.await
			}
			Err(e) => {
				eprintln!(
					"Error getting connection from pool: {}",
					crate::format_error(&e)
				);
				return ControlFlow::Continue(());
			}
		}
	};

	match result {
		Ok(rows) => {
			if rows.is_empty() {
				eprintln!(
					"Did not find any sequence named \"{}.{}\".",
					schema, sequence_name
				);
				return ControlFlow::Continue(());
			}

			let row = &rows[0];
			let start_value: i64 = row.get(0);
			let min_value: i64 = row.get(1);
			let max_value: i64 = row.get(2);
			let increment_by: i64 = row.get(3);
			let is_cycle: bool = row.get(4);
			let cache_size: i64 = row.get(5);

			writer
				.writeln(&format!("Sequence \"{}.{}\"", schema, sequence_name))
				.await;
			writer.writeln("").await;

			let mut table = Table::new();
			crate::table::configure(&mut table);

			table.set_header(vec!["Property", "Value"]);
			table.add_row(vec!["Start value", &start_value.to_string()]);
			table.add_row(vec!["Minimum value", &min_value.to_string()]);
			table.add_row(vec!["Maximum value", &max_value.to_string()]);
			table.add_row(vec!["Increment by", &increment_by.to_string()]);
			table.add_row(vec!["Cycle", if is_cycle { "yes" } else { "no" }]);
			table.add_row(vec!["Cache size", &cache_size.to_string()]);

			if detail {
				let info_result = if sameconn {
					ctx.client
						.query(sequence_info_query, &[&schema, &sequence_name])
						.await
				} else {
					match ctx.pool.get().await {
						Ok(client) => {
							client
								.query(sequence_info_query, &[&schema, &sequence_name])
								.await
						}
						Err(_) => {
							return ControlFlow::Continue(());
						}
					}
				};

				if let Ok(info_rows) = info_result
					&& let Some(info_row) = info_rows.first()
				{
					let owner: String = info_row.get(0);
					let description: Option<String> = info_row.get(1);
					let data_type: String = info_row.get(2);

					table.add_row(vec!["Type", &data_type]);
					table.add_row(vec!["Owner", &owner]);

					if let Some(desc) = description
						&& !desc.is_empty()
					{
						table.add_row(vec!["Description", &desc]);
					}
				}
			}

			crate::table::style_header(&mut table);
			writer.writeln(&format!("{table}")).await;

			let owned_result = if sameconn {
				ctx.client
					.query(owned_by_query, &[&schema, &sequence_name])
					.await
			} else {
				match ctx.pool.get().await {
					Ok(client) => {
						client
							.query(owned_by_query, &[&schema, &sequence_name])
							.await
					}
					Err(_) => {
						return ControlFlow::Continue(());
					}
				}
			};

			if let Ok(owned_rows) = owned_result
				&& let Some(owned_row) = owned_rows.first()
			{
				let owned_by: String = owned_row.get(0);
				writer.writeln(&format!("\nOwned by: {}", owned_by)).await;
			}

			writer.writeln("").await;
			ControlFlow::Continue(())
		}
		Err(e) => {
			eprintln!(
				"Error describing sequence \"{}.{}\": {}",
				schema,
				sequence_name,
				crate::format_error(&e)
			);
			ControlFlow::Continue(())
		}
	}
}
