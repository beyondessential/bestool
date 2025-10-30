use winnow::{
	ascii::{space0, space1},
	combinator::{alt, eof, opt, preceded},
	error::ErrMode,
	token::literal,
	Parser,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListItem {
	Table,
	Index,
	Function,
	View,
	Schema,
	Sequence,
}

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;

	// Parse the base command first
	let alias_type = alt((
		literal("list").map(|_| None),
		literal("dt").map(|_| Some(ListItem::Table)),
		literal("di").map(|_| Some(ListItem::Index)),
		literal("df").map(|_| Some(ListItem::Function)),
		literal("dv").map(|_| Some(ListItem::View)),
		literal("dn").map(|_| Some(ListItem::Schema)),
		literal("ds").map(|_| Some(ListItem::Sequence)),
	))
	.parse_next(input)?;

	// Parse modifiers: + for detail, ! for sameconn (in any order)
	let has_plus_first = opt(literal("+")).parse_next(input)?.is_some();
	let has_exclaim_first = opt(literal("!")).parse_next(input)?.is_some();
	let has_plus_second = opt(literal("+")).parse_next(input)?.is_some();
	let has_exclaim_second = opt(literal("!")).parse_next(input)?.is_some();

	let detail = has_plus_first || has_plus_second;
	let sameconn = has_exclaim_first || has_exclaim_second;

	if let Some(item) = alias_type {
		// For \dt or \di, pattern is optional
		let pattern = opt(preceded(space1, parse_pattern)).parse_next(input)?;
		space0.parse_next(input)?;
		eof.parse_next(input)?;

		Ok(super::Metacommand::List {
			item,
			pattern: pattern.unwrap_or_else(|| "public.*".to_string()),
			detail,
			sameconn,
		})
	} else {
		// For \list, we need the "table" or "index" keyword
		space1.parse_next(input)?;
		let item = alt((
			literal("table").map(|_| ListItem::Table),
			literal("index").map(|_| ListItem::Index),
			literal("function").map(|_| ListItem::Function),
			literal("view").map(|_| ListItem::View),
			literal("schema").map(|_| ListItem::Schema),
			literal("sequence").map(|_| ListItem::Sequence),
		))
		.parse_next(input)?;

		let pattern = opt(preceded(space1, parse_pattern)).parse_next(input)?;
		space0.parse_next(input)?;
		eof.parse_next(input)?;

		Ok(super::Metacommand::List {
			item,
			pattern: pattern.unwrap_or_else(|| "public.*".to_string()),
			detail,
			sameconn,
		})
	}
}

fn parse_pattern(
	input: &mut &str,
) -> winnow::error::Result<String, ErrMode<winnow::error::ContextError>> {
	let start = *input;
	let mut end_pos = 0;

	for (pos, ch) in start.char_indices() {
		if ch.is_whitespace() {
			break;
		}
		end_pos = pos + ch.len_utf8();
	}

	if end_pos == 0 {
		return Err(ErrMode::Backtrack(winnow::error::ContextError::new()));
	}

	let pattern = &start[..end_pos];
	*input = &start[end_pos..];
	Ok(pattern.to_string())
}

#[cfg(test)]
mod tests {
	use super::super::*;
	use super::*;

	#[test]
	fn test_parse_list_table() {
		let result = parse_metacommand("\\list table").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_list_table_with_pattern() {
		let result = parse_metacommand("\\list table users.*").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "users.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_list_plus_table() {
		let result = parse_metacommand("\\list+ table").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_list_plus_table_with_pattern() {
		let result = parse_metacommand("\\list+ table admin.*").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "admin.*".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dt_alias() {
		let result = parse_metacommand("\\dt").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dt_alias_with_pattern() {
		let result = parse_metacommand("\\dt myschema.*").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "myschema.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dt_plus_alias() {
		let result = parse_metacommand("\\dt+").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dt_plus_alias_with_pattern() {
		let result = parse_metacommand("\\dt+ test.*").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "test.*".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_list_with_sameconn() {
		let result = parse_metacommand("\\list! table").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_list_plus_with_sameconn() {
		let result = parse_metacommand("\\list+! table").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_list_sameconn_plus() {
		let result = parse_metacommand("\\list!+ table").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_dt_with_sameconn() {
		let result = parse_metacommand("\\dt!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_dt_plus_with_sameconn() {
		let result = parse_metacommand("\\dt+!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_dt_sameconn_plus() {
		let result = parse_metacommand("\\dt!+").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_list_table_with_whitespace() {
		let result = parse_metacommand("  \\list table  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dt_with_whitespace() {
		let result = parse_metacommand("  \\dt  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Table,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_list_index() {
		let result = parse_metacommand("\\list index").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Index,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_di_alias() {
		let result = parse_metacommand("\\di").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Index,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_di_plus_alias() {
		let result = parse_metacommand("\\di+").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Index,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_di_with_sameconn() {
		let result = parse_metacommand("\\di!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Index,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_di_plus_with_sameconn() {
		let result = parse_metacommand("\\di+!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Index,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_list_function() {
		let result = parse_metacommand("\\list function").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Function,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_df_alias() {
		let result = parse_metacommand("\\df").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Function,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_df_plus_alias() {
		let result = parse_metacommand("\\df+").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Function,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_df_with_sameconn() {
		let result = parse_metacommand("\\df!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Function,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_df_plus_with_sameconn() {
		let result = parse_metacommand("\\df+!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Function,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_list_view() {
		let result = parse_metacommand("\\list view").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::View,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dv_alias() {
		let result = parse_metacommand("\\dv").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::View,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dv_plus_alias() {
		let result = parse_metacommand("\\dv+").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::View,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dv_with_sameconn() {
		let result = parse_metacommand("\\dv!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::View,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_dv_plus_with_sameconn() {
		let result = parse_metacommand("\\dv+!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::View,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_list_schema() {
		let result = parse_metacommand("\\list schema").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Schema,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dn_alias() {
		let result = parse_metacommand("\\dn").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Schema,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dn_plus_alias() {
		let result = parse_metacommand("\\dn+").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Schema,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_dn_with_sameconn() {
		let result = parse_metacommand("\\dn!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Schema,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_dn_plus_with_sameconn() {
		let result = parse_metacommand("\\dn+!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Schema,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_list_sequence() {
		let result = parse_metacommand("\\list sequence").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Sequence,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_ds_alias() {
		let result = parse_metacommand("\\ds").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Sequence,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_ds_plus_alias() {
		let result = parse_metacommand("\\ds+").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Sequence,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_ds_with_sameconn() {
		let result = parse_metacommand("\\ds!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Sequence,
				pattern: "public.*".to_string(),
				detail: false,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_ds_plus_with_sameconn() {
		let result = parse_metacommand("\\ds+!").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::List {
				item: ListItem::Sequence,
				pattern: "public.*".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}
}
