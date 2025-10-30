use std::ops::ControlFlow;

use comfy_table::Table;

use crate::parser::ListItem;

use super::state::ReplContext;

pub async fn handle_list(
	ctx: &mut ReplContext<'_>,
	item: ListItem,
	pattern: String,
	detail: bool,
) -> ControlFlow<()> {
	match item {
		ListItem::Table => handle_list_tables(ctx, &pattern, detail).await,
	}
}

async fn handle_list_tables(
	ctx: &mut ReplContext<'_>,
	pattern: &str,
	detail: bool,
) -> ControlFlow<()> {
	let (schema_pattern, table_pattern) = parse_pattern(pattern);
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
				am.amname AS "Access method",
				CASE
					WHEN c.relacl IS NULL THEN NULL
					ELSE pg_catalog.array_to_string(c.relacl, E'\n')
				END AS "ACL"
			FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			LEFT JOIN pg_catalog.pg_am am ON c.relam = am.oid
			WHERE c.relkind = 'r'
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
				am.amname AS "Access method",
				CASE
					WHEN c.relacl IS NULL THEN NULL
					ELSE pg_catalog.array_to_string(c.relacl, E'\n')
				END AS "ACL"
			FROM pg_catalog.pg_class c
			LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
			LEFT JOIN pg_catalog.pg_am am ON c.relam = am.oid
			WHERE c.relkind = 'r'
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
		WHERE c.relkind = 'r'
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
		WHERE c.relkind = 'r'
			AND n.nspname ~ $1
			AND c.relname ~ $2
		ORDER BY 1, 2
		"#
	};

	let result = ctx
		.client
		.query(query, &[&schema_pattern, &table_pattern])
		.await;

	match result {
		Ok(rows) => {
			if rows.is_empty() {
				println!("No matching tables found.");
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
					"Access method",
					"ACL",
				]);
				for row in rows {
					let schema: String = row.get(0);
					let name: String = row.get(1);
					let size: String = row.get(2);
					let owner: String = row.get(3);
					let persistence: String = row.get(4);
					let access_method: Option<String> = row.get(5);
					let acl: Option<String> = row.get(6);
					table.add_row(vec![
						schema,
						name,
						size,
						owner,
						persistence,
						access_method.unwrap_or_default(),
						acl.unwrap_or_default(),
					]);
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

			println!("{table}");
			ControlFlow::Continue(())
		}
		Err(e) => {
			eprintln!("Error listing tables: {}", e);
			ControlFlow::Continue(())
		}
	}
}

fn should_exclude_system_schemas(pattern: &str) -> bool {
	// If the pattern explicitly mentions information_schema or pg_toast, don't exclude them
	if let Some((schema, _)) = pattern.split_once('.') {
		// Check if schema part explicitly matches these schemas
		schema != "information_schema" && schema != "pg_toast"
	} else {
		// Single word pattern (matches table in public) - exclude system schemas
		pattern != "information_schema" && pattern != "pg_toast"
	}
}

fn parse_pattern(pattern: &str) -> (String, String) {
	if let Some((schema, table)) = pattern.split_once('.') {
		let schema_regex = wildcard_to_regex(schema);
		let table_regex = wildcard_to_regex(table);
		(schema_regex, table_regex)
	} else if pattern == "*" {
		// Special case: * matches all tables in all schemas
		(".*".to_string(), ".*".to_string())
	} else {
		// Pattern without dot matches table name in public schema
		("^public$".to_string(), wildcard_to_regex(pattern))
	}
}

fn wildcard_to_regex(pattern: &str) -> String {
	if pattern == "*" {
		return ".*".to_string();
	}

	let mut regex = String::from("^");
	for ch in pattern.chars() {
		match ch {
			'*' => regex.push_str(".*"),
			'?' => regex.push('.'),
			'.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
				regex.push('\\');
				regex.push(ch);
			}
			_ => regex.push(ch),
		}
	}
	regex.push('$');
	regex
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_pattern_with_schema_and_table() {
		let (schema, table) = parse_pattern("public.users");
		assert_eq!(schema, "^public$");
		assert_eq!(table, "^users$");
	}

	#[test]
	fn test_parse_pattern_with_wildcard_schema() {
		let (schema, table) = parse_pattern("public.*");
		assert_eq!(schema, "^public$");
		assert_eq!(table, ".*");
	}

	#[test]
	fn test_parse_pattern_with_wildcard_table() {
		let (schema, table) = parse_pattern("public.user*");
		assert_eq!(schema, "^public$");
		assert_eq!(table, "^user.*$");
	}

	#[test]
	fn test_parse_pattern_without_dot() {
		let (schema, table) = parse_pattern("users");
		assert_eq!(schema, "^public$");
		assert_eq!(table, "^users$");
	}

	#[test]
	fn test_parse_pattern_star_only() {
		let (schema, table) = parse_pattern("*");
		assert_eq!(schema, ".*");
		assert_eq!(table, ".*");
	}

	#[test]
	fn test_parse_pattern_star_dot_star() {
		let (schema, table) = parse_pattern("*.*");
		assert_eq!(schema, ".*");
		assert_eq!(table, ".*");
	}

	#[test]
	fn test_wildcard_to_regex_star() {
		assert_eq!(wildcard_to_regex("*"), ".*");
	}

	#[test]
	fn test_wildcard_to_regex_literal() {
		assert_eq!(wildcard_to_regex("users"), "^users$");
	}

	#[test]
	fn test_wildcard_to_regex_with_star() {
		assert_eq!(wildcard_to_regex("user*"), "^user.*$");
	}

	#[test]
	fn test_wildcard_to_regex_with_question() {
		assert_eq!(wildcard_to_regex("user?"), "^user.$");
	}

	#[test]
	fn test_wildcard_to_regex_escapes_special_chars() {
		assert_eq!(wildcard_to_regex("test.table"), "^test\\.table$");
	}

	#[test]
	fn test_should_exclude_system_schemas_default() {
		assert!(should_exclude_system_schemas("public.*"));
		assert!(should_exclude_system_schemas("*.*"));
		assert!(should_exclude_system_schemas("users"));
	}

	#[test]
	fn test_should_exclude_system_schemas_explicit() {
		assert!(!should_exclude_system_schemas("information_schema.*"));
		assert!(!should_exclude_system_schemas("pg_toast.*"));
		assert!(!should_exclude_system_schemas("information_schema"));
		assert!(!should_exclude_system_schemas("pg_toast"));
	}
}
