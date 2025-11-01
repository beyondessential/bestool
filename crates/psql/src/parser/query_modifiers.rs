use std::collections::HashSet;

use miette::Result;
use winnow::{
	Parser,
	ascii::{space0, space1},
	combinator::{alt, opt, preceded},
	error::ErrMode,
	token::{literal, rest, take_till},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum QueryModifier {
	Expanded,
	Json,
	Verbatim,
	VarSet { prefix: Option<String> },
	Output { file_path: String },
	Zero,
}

pub(crate) type QueryModifiers = HashSet<QueryModifier>;

pub(crate) fn parse_query_modifiers(input: &str) -> Result<Option<(String, QueryModifiers)>> {
	let input = input.trim();

	fn backslash_g(
		input: &mut &str,
	) -> winnow::error::Result<(), ErrMode<winnow::error::ContextError>> {
		('\\', alt(('g', 'G'))).void().parse_next(input)
	}

	fn modifier_char(
		input: &mut &str,
	) -> winnow::error::Result<char, ErrMode<winnow::error::ContextError>> {
		alt((
			literal('x').map(|_| 'x'),
			literal('j').map(|_| 'j'),
			literal('o').map(|_| 'o'),
			literal('v').map(|_| 'v'),
			literal('z').map(|_| 'z'),
		))
		.parse_next(input)
	}

	fn modifier(
		input: &mut &str,
	) -> winnow::error::Result<
		(Vec<char>, bool, Option<String>),
		ErrMode<winnow::error::ContextError>,
	> {
		backslash_g.parse_next(input)?;

		// Parse zero or more modifier characters (x, j, o)
		let mut modifiers = Vec::new();
		while let Ok(m) = modifier_char.parse_next(input) {
			modifiers.push(m);
		}

		// Try to parse "set"
		let has_set = opt(literal("set")).parse_next(input)?.is_some();

		// If "set" is present, or if 'o' modifier is present, try to parse argument
		let has_output = modifiers.contains(&'o');
		let arg = if has_set || has_output {
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

	type ParseLineResult<'a> = winnow::error::Result<
		(&'a str, Option<(Vec<char>, bool, Option<String>)>),
		ErrMode<winnow::error::ContextError>,
	>;

	fn parse_line<'a>(input: &mut &'a str) -> ParseLineResult<'a> {
		let sql = take_till(1.., |c| c == '\\').parse_next(input)?;
		let cmd_and_arg = opt((space0, modifier)).parse_next(input)?;
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
			let has_output = modifier_chars.contains(&'o');

			// Apply modifiers based on the characters we found
			for ch in &modifier_chars {
				match ch {
					'x' => {
						modifiers.insert(QueryModifier::Expanded);
					}
					'j' => {
						modifiers.insert(QueryModifier::Json);
					}
					'v' => {
						modifiers.insert(QueryModifier::Verbatim);
					}
					'o' => {
						// Output modifier needs a file path argument
						if let Some(ref file_path) = arg {
							modifiers.insert(QueryModifier::Output {
								file_path: file_path.clone(),
							});
						}
					}
					'z' => {
						modifiers.insert(QueryModifier::Zero);
					}
					_ => {}
				}
			}

			// Apply set modifier if present
			// Note: 'set' and 'o' cannot both be present (arg is used for both)
			if has_set && !has_output {
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
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_backslash_no_space() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gj").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_backslash_g() {
		let result = parse_query_modifiers(r"SELECT * FROM users \g").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_gx() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_gset() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gset").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet { prefix: None }));
	}

	#[test]
	fn test_parse_query_modifiers_gset_with_prefix() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gset myprefix").unwrap();
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
		let result = parse_query_modifiers(r"SELECT * FROM users \gxset").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet { prefix: None }));
	}

	#[test]
	fn test_parse_query_modifiers_gxset_with_prefix() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxset myprefix").unwrap();
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
		let result = parse_query_modifiers(r"  SELECT * FROM users  \gx  ").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_multiline() {
		let result =
			parse_query_modifiers("SELECT *\nFROM users\nWHERE id = 1 \\gset var").unwrap();
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
		let result = parse_query_modifiers(r"SELECT count(*) FROM users \gset my_prefix_").unwrap();
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
		let result = parse_query_modifiers(r"SELECT * FROM users \Gx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_case_insensitive_gset() {
		let result = parse_query_modifiers(r"SELECT * FROM users \Gset prefix").unwrap();
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
	fn test_parse_query_modifiers_gxset_prefix_no_space() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxsetprefix").unwrap();
		assert!(result.is_none());
	}

	#[test]
	fn test_parse_query_modifiers_gj() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gj").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_gjx() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gjx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_gxj() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxj").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_gjset() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gjset").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::VarSet { prefix: None }));
	}

	#[test]
	fn test_parse_query_modifiers_gxjset() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxjset var").unwrap();
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
		let result = parse_query_modifiers(r"SELECT * FROM users \gjxset prefix").unwrap();
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
	fn test_parse_query_modifiers_wrong_case() {
		let result = parse_query_modifiers(r"SELECT * FROM users \GJ").unwrap();
		assert!(result.is_none());
	}

	#[test]
	fn test_parse_query_modifiers_duplicate_modifiers() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(!mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_duplicate_mixed() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gjjx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::VarSet { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_all_modifiers() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxjset myvar").unwrap();
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
	fn test_parse_query_modifiers_no_terminator() {
		let result = parse_query_modifiers("SELECT * FROM users").unwrap();
		assert!(result.is_none());
	}

	#[test]
	fn test_parse_query_modifiers_go() {
		let result = parse_query_modifiers(r"SELECT * FROM users \go /tmp/output.txt").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Output {
			file_path: "/tmp/output.txt".to_string()
		}));
	}

	#[test]
	fn test_parse_query_modifiers_go_relative_path() {
		let result = parse_query_modifiers(r"SELECT 1 \go ./output/result.txt").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT 1");
		assert!(mods.contains(&QueryModifier::Output {
			file_path: "./output/result.txt".to_string()
		}));
	}

	#[test]
	fn test_parse_query_modifiers_go_uppercase() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gO /tmp/output.txt").unwrap();
		assert!(result.is_none());
	}

	#[test]
	fn test_parse_query_modifiers_gxo() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxo /tmp/output.txt").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Output {
			file_path: "/tmp/output.txt".to_string()
		}));
	}

	#[test]
	fn test_parse_query_modifiers_gjo() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gjo /tmp/output.json").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::Output {
			file_path: "/tmp/output.json".to_string()
		}));
	}

	#[test]
	fn test_parse_query_modifiers_gxjo() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxjo /tmp/output.json").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Json));
		assert!(mods.contains(&QueryModifier::Output {
			file_path: "/tmp/output.json".to_string()
		}));
	}

	#[test]
	fn test_parse_query_modifiers_go_without_path() {
		let result = parse_query_modifiers(r"SELECT * FROM users \go").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		// Should not contain Output modifier if no path provided
		assert!(
			!mods
				.iter()
				.any(|m| matches!(m, QueryModifier::Output { .. }))
		);
	}

	#[test]
	fn test_parse_query_modifiers_gv() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gv").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Verbatim));
	}

	#[test]
	fn test_parse_query_modifiers_gvx() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gvx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Verbatim));
		assert!(mods.contains(&QueryModifier::Expanded));
	}

	#[test]
	fn test_parse_query_modifiers_gxv() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxv").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Verbatim));
		assert!(mods.contains(&QueryModifier::Expanded));
	}

	#[test]
	fn test_parse_query_modifiers_gz() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gz").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Zero));
	}

	#[test]
	fn test_parse_query_modifiers_gzx() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gzx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Zero));
		assert!(mods.contains(&QueryModifier::Expanded));
	}

	#[test]
	fn test_parse_query_modifiers_gxz() {
		let result = parse_query_modifiers(r"SELECT * FROM users \gxz").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Zero));
		assert!(mods.contains(&QueryModifier::Expanded));
	}
}
