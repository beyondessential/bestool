use miette::Result;
use std::collections::HashSet;
use winnow::ascii::{space0, space1, Caseless};
use winnow::combinator::{alt, opt, preceded};
use winnow::error::ErrMode;
use winnow::token::{literal, rest, take_till};
use winnow::Parser;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum QueryModifier {
	Expanded,
	Json,
	VarSet { prefix: Option<String> },
}

pub(crate) type QueryModifiers = HashSet<QueryModifier>;

pub(crate) fn parse_query_modifiers(input: &str) -> Result<Option<(String, QueryModifiers)>> {
	let input = input.trim();

	fn backslash_g<'a>(
		input: &mut &'a str,
	) -> winnow::error::Result<(), ErrMode<winnow::error::ContextError>> {
		('\\', alt(('g', 'G'))).void().parse_next(input)
	}

	fn modifier_char<'a>(
		input: &mut &'a str,
	) -> winnow::error::Result<char, ErrMode<winnow::error::ContextError>> {
		alt((alt(('x', 'X')).map(|_| 'x'), alt(('j', 'J')).map(|_| 'j'))).parse_next(input)
	}

	fn metacommand<'a>(
		input: &mut &'a str,
	) -> winnow::error::Result<
		(Vec<char>, bool, Option<String>),
		ErrMode<winnow::error::ContextError>,
	> {
		let _ = backslash_g.parse_next(input)?;

		// Parse zero or more modifier characters (x, j)
		let mut modifiers = Vec::new();
		while let Ok(m) = modifier_char.parse_next(input) {
			modifiers.push(m);
		}

		// Try to parse "set"
		let has_set = opt(literal(Caseless("set"))).parse_next(input)?.is_some();

		// If "set" is present, try to parse optional prefix
		let arg = if has_set {
			opt(preceded(space1, rest.map(|s: &str| s.trim())))
				.parse_next(input)?
				.and_then(|s| {
					if s.is_empty() {
						None
					} else {
						Some(s.to_string())
					}
				})
		} else {
			None
		};

		Ok((modifiers, has_set, arg))
	}

	fn parse_line<'a>(
		input: &mut &'a str,
	) -> winnow::error::Result<
		(&'a str, Option<(Vec<char>, bool, Option<String>)>),
		ErrMode<winnow::error::ContextError>,
	> {
		let sql = take_till(1.., |c| c == '\\').parse_next(input)?;
		let cmd_and_arg = opt((space0, metacommand)).parse_next(input)?;
		Ok((sql, cmd_and_arg.map(|(_, cmd)| cmd)))
	}

	// First check if input ends with semicolon
	if input.trim_end().ends_with(';') {
		let sql = input.trim_end().trim_end_matches(';').trim().to_string();
		return Ok(Some((sql, QueryModifiers::new())));
	}

	// Try to parse metacommand
	match parse_line.parse(input) {
		Ok((sql, Some((modifier_chars, has_set, arg)))) => {
			let mut modifiers = QueryModifiers::new();

			// Apply modifiers based on the characters we found
			for ch in modifier_chars {
				match ch {
					'x' => {
						modifiers.insert(QueryModifier::Expanded);
					}
					'j' => {
						modifiers.insert(QueryModifier::Json);
					}
					_ => {}
				}
			}

			// Apply set modifier if present
			if has_set {
				modifiers.insert(QueryModifier::VarSet { prefix: arg });
			}

			Ok(Some((sql.trim().to_string(), modifiers)))
		}
		Ok((_, None)) | Err(_) => Ok(None),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_query_modifiers_semicolon() {
		let result = parse_query_modifiers("SELECT * FROM users;").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_backslash_g() {
		let result = parse_query_modifiers("SELECT * FROM users\\g").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_gx() {
		let result = parse_query_modifiers("SELECT * FROM users\\gx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_gset() {
		let result = parse_query_modifiers("SELECT * FROM users\\gset").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet { prefix: None }));
	}

	#[test]
	fn test_parse_query_modifiers_gset_with_prefix() {
		let result = parse_query_modifiers("SELECT * FROM users\\gset myprefix").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("myprefix".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_gxset() {
		let result = parse_query_modifiers("SELECT * FROM users\\gxset").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet { prefix: None }));
	}

	#[test]
	fn test_parse_query_modifiers_gxset_with_prefix() {
		let result = parse_query_modifiers("SELECT * FROM users\\gxset myprefix").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("myprefix".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_with_whitespace() {
		let result = parse_query_modifiers("  SELECT * FROM users  \\gx  ").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_multiline() {
		let result = parse_query_modifiers("SELECT *\nFROM users\nWHERE id = 1\\gset var").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT *\nFROM users\nWHERE id = 1");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("var".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_prefix_with_underscore() {
		let result = parse_query_modifiers("SELECT count(*) FROM users\\gset my_prefix_").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT count(*) FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("my_prefix_".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_case_insensitive_gx() {
		let result = parse_query_modifiers("SELECT * FROM users\\GX").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_case_insensitive_gset() {
		let result = parse_query_modifiers("SELECT * FROM users\\Gset prefix").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("prefix".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_case_insensitive_gxset() {
		let result = parse_query_modifiers("SELECT * FROM users\\GXSET myvar").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("myvar".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_gxset_prefix_no_space() {
		let result = parse_query_modifiers("SELECT * FROM users\\gxsetprefix").unwrap();
		assert!(result.is_none());
	}

	#[test]
	fn test_parse_query_modifiers_gj() {
		let result = parse_query_modifiers("SELECT * FROM users\\gj").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_gjx() {
		let result = parse_query_modifiers("SELECT * FROM users\\gjx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_gxj() {
		let result = parse_query_modifiers("SELECT * FROM users\\gxj").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_gjset() {
		let result = parse_query_modifiers("SELECT * FROM users\\gjset").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet { prefix: None }));
	}

	#[test]
	fn test_parse_query_modifiers_gxjset() {
		let result = parse_query_modifiers("SELECT * FROM users\\gxjset var").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("var".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_gjxset() {
		let result = parse_query_modifiers("SELECT * FROM users\\gjxset prefix").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("prefix".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_case_insensitive_gj() {
		let result = parse_query_modifiers("SELECT * FROM users\\GJ").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_duplicate_modifiers() {
		let result = parse_query_modifiers("SELECT * FROM users\\gxx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_duplicate_mixed() {
		let result = parse_query_modifiers("SELECT * FROM users\\gjjx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::VarSet { .. })));
	}

	#[test]
	fn test_parse_query_modifiers_all_modifiers() {
		let result = parse_query_modifiers("SELECT * FROM users\\gxjset myvar").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("myvar".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_mixed_case_modifiers() {
		let result = parse_query_modifiers("SELECT * FROM users\\GxJsEt var").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet {
			prefix: Some("var".to_string())
		}));
	}

	#[test]
	fn test_parse_query_modifiers_no_terminator() {
		let result = parse_query_modifiers("SELECT * FROM users").unwrap();
		assert!(result.is_none());
	}
}
