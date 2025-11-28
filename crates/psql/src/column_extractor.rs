use std::collections::HashSet;

use miette::Result;
use pg_query::{NodeEnum, parse};
use tracing::debug;

use crate::schema_cache::SchemaCache;

/// A tuple representing (schema, table, column)
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ColumnRef {
	pub schema: String,
	pub table: String,
	pub column: String,
}

/// Extract column references from a SQL query
pub fn extract_column_refs(
	sql: &str,
	schema_cache: Option<&SchemaCache>,
) -> Result<Vec<ColumnRef>> {
	// Parse the SQL using pg_query
	let parse_result = match parse(sql) {
		Ok(result) => result,
		Err(e) => {
			debug!("Failed to parse SQL for column extraction: {}", e);
			return Ok(Vec::new());
		}
	};

	let mut column_refs = Vec::new();
	let mut context = ExtractionContext {
		schema_cache,
		column_refs: &mut column_refs,
		table_aliases: Default::default(),
		in_select_list: false,
	};

	// Process each statement in the parse tree
	for statement in parse_result.protobuf.stmts {
		if let Some(stmt) = statement.stmt {
			process_node(&stmt.node, &mut context);
		}
	}

	// Deduplicate while preserving order
	let mut seen = HashSet::new();
	column_refs.retain(|col_ref| seen.insert(col_ref.clone()));

	Ok(column_refs)
}

struct ExtractionContext<'a> {
	schema_cache: Option<&'a SchemaCache>,
	column_refs: &'a mut Vec<ColumnRef>,
	table_aliases: std::collections::HashMap<String, (String, String)>,
	in_select_list: bool,
}

fn process_node(node: &Option<NodeEnum>, ctx: &mut ExtractionContext<'_>) {
	let Some(node) = node else { return };

	match node {
		NodeEnum::SelectStmt(select) => {
			// First, process FROM clause to build table aliases
			for from_item in &select.from_clause {
				if let Some(NodeEnum::RangeVar(range)) = &from_item.node {
					let table_name = range.relname.clone();
					let schema_name = if range.schemaname.is_empty() {
						if let Some(cache) = ctx.schema_cache {
							find_schema_for_table(cache, &table_name)
								.unwrap_or_else(|| "public".to_string())
						} else {
							"public".to_string()
						}
					} else {
						range.schemaname.clone()
					};

					let alias = if let Some(a) = &range.alias {
						a.aliasname.clone()
					} else {
						table_name.clone()
					};

					ctx.table_aliases.insert(alias, (schema_name, table_name));
				}
				// Process other types of FROM items (subqueries, joins, etc)
				process_from_item(&from_item.node, ctx);
			}

			// Process target list (SELECT items)
			let old_in_select_list = ctx.in_select_list;
			ctx.in_select_list = true;

			for target in &select.target_list {
				if let Some(NodeEnum::ResTarget(res)) = &target.node
					&& let Some(val) = &res.val
				{
					// Check if this is a simple ColumnRef (not a computed expression)
					if let Some(NodeEnum::ColumnRef(_)) = &val.node {
						process_node(&val.node, ctx);
					} else if let Some(NodeEnum::AStar(_)) = &val.node {
						// SELECT * - expand to all columns
						expand_wildcard(None, ctx);
					}
					// For other expressions (computed columns), we don't extract
				}
			}

			ctx.in_select_list = old_in_select_list;

			// Process WHERE clause
			if let Some(where_clause) = &select.where_clause {
				process_node(&where_clause.node, ctx);
			}

			// Process GROUP BY
			for group in &select.group_clause {
				process_node(&group.node, ctx);
			}

			// Process HAVING
			if let Some(having) = &select.having_clause {
				process_node(&having.node, ctx);
			}
		}
		NodeEnum::ColumnRef(col_ref) => {
			// Extract column reference
			process_column_ref(col_ref, ctx);
		}
		NodeEnum::AStar(_) => {
			// SELECT * or table.*
			expand_wildcard(None, ctx);
		}
		NodeEnum::RangeVar(_) => {
			// Already handled in FROM processing
		}
		NodeEnum::JoinExpr(join) => {
			// Process both sides of the join
			if let Some(larg) = &join.larg {
				process_node(&larg.node, ctx);
			}
			if let Some(rarg) = &join.rarg {
				process_node(&rarg.node, ctx);
			}
			// Process join condition
			if let Some(quals) = &join.quals {
				process_node(&quals.node, ctx);
			}
		}
		NodeEnum::AExpr(expr) => {
			// Binary/unary expressions - process operands but don't mark as direct refs
			let old_in_select_list = ctx.in_select_list;
			ctx.in_select_list = false;

			if let Some(lexpr) = &expr.lexpr {
				process_node(&lexpr.node, ctx);
			}
			if let Some(rexpr) = &expr.rexpr {
				process_node(&rexpr.node, ctx);
			}

			ctx.in_select_list = old_in_select_list;
		}
		NodeEnum::BoolExpr(expr) => {
			// Boolean expressions (AND, OR, NOT)
			for arg in &expr.args {
				process_node(&arg.node, ctx);
			}
		}
		NodeEnum::FuncCall(_) => {
			// Function calls are computed expressions, don't extract columns from them
			// even though they might reference columns
		}
		NodeEnum::SubLink(sublink) => {
			// Subquery - process it
			if let Some(subselect) = &sublink.subselect {
				process_node(&subselect.node, ctx);
			}
		}
		NodeEnum::RangeSubselect(range_sub) => {
			// Subquery in FROM clause
			if let Some(subquery) = &range_sub.subquery {
				process_node(&subquery.node, ctx);
			}
		}
		_ => {
			// For other node types, we don't need to extract columns
		}
	}
}

fn process_from_item(node: &Option<NodeEnum>, ctx: &mut ExtractionContext<'_>) {
	let Some(node) = node else { return };

	match node {
		NodeEnum::RangeVar(range) => {
			let table_name = range.relname.clone();
			let schema_name = if range.schemaname.is_empty() {
				if let Some(cache) = ctx.schema_cache {
					find_schema_for_table(cache, &table_name)
						.unwrap_or_else(|| "public".to_string())
				} else {
					"public".to_string()
				}
			} else {
				range.schemaname.clone()
			};

			let alias = if let Some(a) = &range.alias {
				a.aliasname.clone()
			} else {
				table_name.clone()
			};

			ctx.table_aliases.insert(alias, (schema_name, table_name));
		}
		NodeEnum::JoinExpr(join) => {
			if let Some(larg) = &join.larg {
				process_from_item(&larg.node, ctx);
			}
			if let Some(rarg) = &join.rarg {
				process_from_item(&rarg.node, ctx);
			}
		}
		NodeEnum::RangeSubselect(_) => {
			// Subquery - we could track this but for now skip
		}
		_ => {}
	}
}

fn process_column_ref(col_ref: &pg_query::protobuf::ColumnRef, ctx: &mut ExtractionContext<'_>) {
	let fields: Vec<String> = col_ref
		.fields
		.iter()
		.filter_map(|field| {
			if let Some(NodeEnum::String(s)) = &field.node {
				Some(s.sval.clone())
			} else if let Some(NodeEnum::AStar(_)) = &field.node {
				None // Handle * separately
			} else {
				None
			}
		})
		.collect();

	// Check if this is a wildcard (table.* or just *)
	let has_star = col_ref
		.fields
		.iter()
		.any(|field| matches!(&field.node, Some(NodeEnum::AStar(_))));

	if has_star {
		if !fields.is_empty() {
			// table.* case
			let table_name = &fields[0];
			expand_wildcard(Some(table_name), ctx);
		} else {
			// SELECT * case (unqualified wildcard)
			expand_wildcard(None, ctx);
		}
		return;
	}

	match fields.len() {
		1 => {
			// Simple column reference (no table qualifier)
			let column_name = &fields[0];

			// If there's only one table, use it
			if ctx.table_aliases.len() == 1
				&& let Some((schema, table)) = ctx.table_aliases.values().next()
			{
				ctx.column_refs.push(ColumnRef {
					schema: schema.clone(),
					table: table.clone(),
					column: column_name.clone(),
				});
			}
			// Otherwise, we can't determine which table without more analysis
		}
		2 => {
			// table.column
			let table_or_alias = &fields[0];
			let column_name = &fields[1];

			if let Some((schema, table)) = ctx.table_aliases.get(table_or_alias) {
				ctx.column_refs.push(ColumnRef {
					schema: schema.clone(),
					table: table.clone(),
					column: column_name.clone(),
				});
			}
		}
		3 => {
			// schema.table.column
			let schema = &fields[0];
			let table = &fields[1];
			let column = &fields[2];
			ctx.column_refs.push(ColumnRef {
				schema: schema.clone(),
				table: table.clone(),
				column: column.clone(),
			});
		}
		_ => {}
	}
}

fn expand_wildcard(table_qualifier: Option<&str>, ctx: &mut ExtractionContext<'_>) {
	let Some(cache) = ctx.schema_cache else {
		return;
	};

	if let Some(table_name) = table_qualifier {
		// Expand table.*
		if let Some((schema, table)) = ctx.table_aliases.get(table_name)
			&& let Some(columns) = cache.columns_for_table(table)
		{
			for column in columns {
				ctx.column_refs.push(ColumnRef {
					schema: schema.clone(),
					table: table.clone(),
					column: column.clone(),
				});
			}
		}
	} else {
		// Expand * - all columns from all tables
		for (schema, table) in ctx.table_aliases.values() {
			if let Some(columns) = cache.columns_for_table(table) {
				for column in columns {
					ctx.column_refs.push(ColumnRef {
						schema: schema.clone(),
						table: table.clone(),
						column: column.clone(),
					});
				}
			}
		}
	}
}

fn find_schema_for_table(cache: &SchemaCache, table: &str) -> Option<String> {
	// First check if it exists in public schema
	if cache
		.tables
		.get("public")
		.is_some_and(|tables| tables.contains(&table.to_string()))
	{
		return Some("public".to_string());
	}

	// Check other schemas
	for (schema_name, tables) in &cache.tables {
		if tables.contains(&table.to_string()) {
			return Some(schema_name.clone());
		}
	}

	None
}

#[cfg(test)]
mod tests {
	use super::*;

	fn create_test_cache() -> SchemaCache {
		let mut cache = SchemaCache::new();
		cache
			.tables
			.insert("public".to_string(), vec!["patient".to_string()]);
		cache.columns.insert(
			"public.patient".to_string(),
			vec!["foo".to_string(), "bar".to_string(), "baz".to_string()],
		);
		cache.columns.insert(
			"patient".to_string(),
			vec!["foo".to_string(), "bar".to_string(), "baz".to_string()],
		);
		cache
	}

	#[test]
	fn test_parse_structure() {
		let sql = "SELECT * FROM patient";
		let result = pg_query::parse(sql).unwrap();

		// Print the structure to understand it
		for stmt in &result.protobuf.stmts {
			if let Some(s) = &stmt.stmt {
				if let Some(pg_query::NodeEnum::SelectStmt(select)) = &s.node {
					eprintln!("Target list length: {}", select.target_list.len());
					for (i, target) in select.target_list.iter().enumerate() {
						eprintln!("Target {}: {:?}", i, target.node);
					}
				}
			}
		}
	}

	#[test]
	fn test_simple_select() {
		let cache = create_test_cache();
		let sql = "SELECT foo, bar FROM patient WHERE bar = 123";
		let refs = extract_column_refs(sql, Some(&cache)).unwrap();

		assert_eq!(refs.len(), 2);
		assert!(refs.contains(&ColumnRef {
			schema: "public".into(),
			table: "patient".into(),
			column: "foo".into()
		}));
		assert!(refs.contains(&ColumnRef {
			schema: "public".into(),
			table: "patient".into(),
			column: "bar".into()
		}));
	}

	#[test]
	fn test_select_with_expression() {
		let cache = create_test_cache();
		let sql = "SELECT bar, foo + 2 FROM patient";
		let refs = extract_column_refs(sql, Some(&cache)).unwrap();

		// Should only return 'bar', not 'foo' because it's part of an expression
		assert_eq!(refs.len(), 1);
		assert!(refs.contains(&ColumnRef {
			schema: "public".into(),
			table: "patient".into(),
			column: "bar".into()
		}));
	}

	#[test]
	fn test_select_star() {
		let cache = create_test_cache();
		let sql = "SELECT * FROM patient";
		let refs = extract_column_refs(sql, Some(&cache)).unwrap();

		assert_eq!(refs.len(), 3);
		assert!(refs.contains(&ColumnRef {
			schema: "public".into(),
			table: "patient".into(),
			column: "foo".into()
		}));
		assert!(refs.contains(&ColumnRef {
			schema: "public".into(),
			table: "patient".into(),
			column: "bar".into()
		}));
		assert!(refs.contains(&ColumnRef {
			schema: "public".into(),
			table: "patient".into(),
			column: "baz".into()
		}));
	}

	#[test]
	fn test_select_qualified_columns() {
		let cache = create_test_cache();
		let sql = "SELECT patient.foo, patient.bar FROM patient";
		let refs = extract_column_refs(sql, Some(&cache)).unwrap();

		assert_eq!(refs.len(), 2);
		assert!(refs.contains(&ColumnRef {
			schema: "public".into(),
			table: "patient".into(),
			column: "foo".into()
		}));
		assert!(refs.contains(&ColumnRef {
			schema: "public".into(),
			table: "patient".into(),
			column: "bar".into()
		}));
	}
}
