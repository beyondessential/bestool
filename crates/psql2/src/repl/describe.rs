use std::ops::ControlFlow;

use crate::repl::state::ReplContext;

mod index;
mod sequence;
mod table;
mod view;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RelationKind {
	Table,
	View,
	MaterializedView,
	Index,
	Sequence,
	CompositeType,
	ForeignTable,
	PartitionedTable,
	PartitionedIndex,
}

impl RelationKind {
	fn from_relkind(relkind: char) -> Option<Self> {
		match relkind {
			'r' => Some(Self::Table),
			'v' => Some(Self::View),
			'm' => Some(Self::MaterializedView),
			'i' => Some(Self::Index),
			'S' => Some(Self::Sequence),
			'c' => Some(Self::CompositeType),
			'f' => Some(Self::ForeignTable),
			'p' => Some(Self::PartitionedTable),
			'I' => Some(Self::PartitionedIndex),
			_ => None,
		}
	}
}

pub(super) fn parse_item(item: &str) -> (String, String) {
	if let Some((schema, name)) = item.split_once('.') {
		(schema.to_string(), name.to_string())
	} else {
		("public".to_string(), item.to_string())
	}
}

pub async fn handle_describe(
	ctx: &mut ReplContext<'_>,
	item: String,
	detail: bool,
	sameconn: bool,
) -> ControlFlow<()> {
	let (schema, name) = parse_item(&item);

	let query = r#"
		SELECT
			n.nspname AS schema_name,
			c.relname AS relation_name,
			c.relkind::text AS relation_kind,
			c.oid AS relation_oid
		FROM pg_catalog.pg_class c
		LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
		WHERE n.nspname = $1
			AND c.relname = $2
	"#;

	let result = if sameconn {
		ctx.client.query(query, &[&schema, &name]).await
	} else {
		match ctx.pool.get().await {
			Ok(client) => client.query(query, &[&schema, &name]).await,
			Err(e) => {
				eprintln!("Error getting connection from pool: {}", e);
				return ControlFlow::Continue(());
			}
		}
	};

	match result {
		Ok(rows) => {
			if rows.is_empty() {
				eprintln!("Did not find any relation named \"{}\".", item);
				return ControlFlow::Continue(());
			}

			let row = &rows[0];
			let relkind_str: String = row.get(2);
			let relkind_char: char = relkind_str.chars().next().unwrap();
			let relation_kind = RelationKind::from_relkind(relkind_char);

			match relation_kind {
				Some(RelationKind::Table | RelationKind::PartitionedTable) => {
					table::handle_describe_table(ctx, &schema, &name, detail, sameconn).await
				}
				Some(RelationKind::View | RelationKind::MaterializedView) => {
					view::handle_describe_view(ctx, &schema, &name, detail, sameconn).await
				}
				Some(RelationKind::Index | RelationKind::PartitionedIndex) => {
					index::handle_describe_index(ctx, &schema, &name, detail, sameconn).await
				}
				Some(RelationKind::Sequence) => {
					sequence::handle_describe_sequence(ctx, &schema, &name, detail, sameconn).await
				}
				Some(RelationKind::CompositeType) => {
					eprintln!("Composite types are not yet supported for describe.");
					ControlFlow::Continue(())
				}
				Some(RelationKind::ForeignTable) => {
					eprintln!("Foreign tables are not yet supported for describe.");
					ControlFlow::Continue(())
				}
				None => {
					eprintln!("Unknown relation kind for \"{}\".", item);
					ControlFlow::Continue(())
				}
			}
		}
		Err(e) => {
			eprintln!("Error describing relation: {}", e);
			ControlFlow::Continue(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_item_with_schema() {
		let (schema, name) = parse_item("myschema.mytable");
		assert_eq!(schema, "myschema");
		assert_eq!(name, "mytable");
	}

	#[test]
	fn test_parse_item_without_schema() {
		let (schema, name) = parse_item("mytable");
		assert_eq!(schema, "public");
		assert_eq!(name, "mytable");
	}

	#[test]
	fn test_relkind_from_char() {
		assert_eq!(RelationKind::from_relkind('r'), Some(RelationKind::Table));
		assert_eq!(RelationKind::from_relkind('v'), Some(RelationKind::View));
		assert_eq!(RelationKind::from_relkind('i'), Some(RelationKind::Index));
		assert_eq!(
			RelationKind::from_relkind('S'),
			Some(RelationKind::Sequence)
		);
		assert_eq!(RelationKind::from_relkind('x'), None);
	}
}
