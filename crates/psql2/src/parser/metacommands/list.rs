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
}

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;

	// Try to parse \list[+][!] or \dt[+][!] or \di[+][!]
	let (detail, sameconn, alias_type) = alt((
		// \list+!
		literal("list+!").map(|_| (true, true, None)),
		// \list!+
		literal("list!+").map(|_| (true, true, None)),
		// \list+
		literal("list+").map(|_| (true, false, None)),
		// \list!
		literal("list!").map(|_| (false, true, None)),
		// \list
		literal("list").map(|_| (false, false, None)),
		// \dt+!
		literal("dt+!").map(|_| (true, true, Some(ListItem::Table))),
		// \dt!+
		literal("dt!+").map(|_| (true, true, Some(ListItem::Table))),
		// \dt+
		literal("dt+").map(|_| (true, false, Some(ListItem::Table))),
		// \dt!
		literal("dt!").map(|_| (false, true, Some(ListItem::Table))),
		// \dt
		literal("dt").map(|_| (false, false, Some(ListItem::Table))),
		// \di+!
		literal("di+!").map(|_| (true, true, Some(ListItem::Index))),
		// \di!+
		literal("di!+").map(|_| (true, true, Some(ListItem::Index))),
		// \di+
		literal("di+").map(|_| (true, false, Some(ListItem::Index))),
		// \di!
		literal("di!").map(|_| (false, true, Some(ListItem::Index))),
		// \di
		literal("di").map(|_| (false, false, Some(ListItem::Index))),
	))
	.parse_next(input)?;

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
}
