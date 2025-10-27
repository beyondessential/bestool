use winnow::ascii::{space0, space1, Caseless};
use winnow::combinator::{alt, opt, preceded};
use winnow::error::ErrMode;
use winnow::token::{literal, rest, take_till};
use winnow::Parser;

#[derive(Debug, Clone, Default)]
pub struct QueryModifiers {
	pub expanded: bool,
	pub varset: bool,
	pub prefix: Option<String>,
}

pub fn parse_query_modifiers(input: &str) -> (String, QueryModifiers) {
	let input = input.trim();

	fn backslash_g<'a>(
		input: &mut &'a str,
	) -> winnow::error::Result<(), ErrMode<winnow::error::ContextError>> {
		('\\', alt(('g', 'G'))).void().parse_next(input)
	}

	fn metacommand<'a>(
		input: &mut &'a str,
	) -> winnow::error::Result<(String, Option<String>), ErrMode<winnow::error::ContextError>> {
		let _ = backslash_g.parse_next(input)?;

		let cmd = alt((
			literal(Caseless("xset")).map(|_| "xset".to_string()),
			literal(Caseless("set")).map(|_| "set".to_string()),
			literal(Caseless("x")).map(|_| "x".to_string()),
			literal("").map(|_| "".to_string()),
		))
		.parse_next(input)?;

		let arg = opt(preceded(space1, rest.map(|s: &str| s.trim())))
			.parse_next(input)?
			.and_then(|s| {
				if s.is_empty() {
					None
				} else {
					Some(s.to_string())
				}
			});

		Ok((cmd, arg))
	}

	fn parse_line<'a>(
		input: &mut &'a str,
	) -> winnow::error::Result<
		(&'a str, Option<(String, Option<String>)>),
		ErrMode<winnow::error::ContextError>,
	> {
		let sql = take_till(1.., |c| c == '\\').parse_next(input)?;
		let cmd_and_arg = opt((space0, metacommand)).parse_next(input)?;
		Ok((sql, cmd_and_arg.map(|(_, cmd)| cmd)))
	}

	match parse_line.parse(input) {
		Ok((sql, Some((cmd, arg)))) => {
			let mut modifiers = QueryModifiers::default();
			match cmd.as_str() {
				"xset" => {
					modifiers.expanded = true;
					modifiers.varset = true;
					modifiers.prefix = arg;
				}
				"set" => {
					modifiers.varset = true;
					modifiers.prefix = arg;
				}
				"x" => {
					modifiers.expanded = true;
				}
				_ => {}
			}
			(sql.trim().to_string(), modifiers)
		}
		Ok((sql, None)) => (sql.trim().to_string(), QueryModifiers::default()),
		Err(_) => (input.to_string(), QueryModifiers::default()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_query_modifiers_semicolon() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users;");
		assert_eq!(sql, "SELECT * FROM users;");
		assert!(!mods.expanded);
		assert!(!mods.varset);
		assert_eq!(mods.prefix, None);
	}

	#[test]
	fn test_parse_query_modifiers_backslash_g() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\g");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.expanded);
		assert!(!mods.varset);
		assert_eq!(mods.prefix, None);
	}

	#[test]
	fn test_parse_query_modifiers_gx() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\gx");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.expanded);
		assert!(!mods.varset);
		assert_eq!(mods.prefix, None);
	}

	#[test]
	fn test_parse_query_modifiers_gset() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\gset");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.expanded);
		assert!(mods.varset);
		assert_eq!(mods.prefix, None);
	}

	#[test]
	fn test_parse_query_modifiers_gset_with_prefix() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\gset myprefix");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.expanded);
		assert!(mods.varset);
		assert_eq!(mods.prefix, Some("myprefix".to_string()));
	}

	#[test]
	fn test_parse_query_modifiers_gxset() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\gxset");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.expanded);
		assert!(mods.varset);
		assert_eq!(mods.prefix, None);
	}

	#[test]
	fn test_parse_query_modifiers_gxset_with_prefix() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\gxset myprefix");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.expanded);
		assert!(mods.varset);
		assert_eq!(mods.prefix, Some("myprefix".to_string()));
	}

	#[test]
	fn test_parse_query_modifiers_with_whitespace() {
		let (sql, mods) = parse_query_modifiers("  SELECT * FROM users  \\gx  ");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.expanded);
		assert!(!mods.varset);
	}

	#[test]
	fn test_parse_query_modifiers_multiline() {
		let (sql, mods) = parse_query_modifiers("SELECT *\nFROM users\nWHERE id = 1\\gset var");
		assert_eq!(sql, "SELECT *\nFROM users\nWHERE id = 1");
		assert!(!mods.expanded);
		assert!(mods.varset);
		assert_eq!(mods.prefix, Some("var".to_string()));
	}

	#[test]
	fn test_parse_query_modifiers_prefix_with_underscore() {
		let (sql, mods) = parse_query_modifiers("SELECT count(*) FROM users\\gset my_prefix_");
		assert_eq!(sql, "SELECT count(*) FROM users");
		assert!(!mods.expanded);
		assert!(mods.varset);
		assert_eq!(mods.prefix, Some("my_prefix_".to_string()));
	}

	#[test]
	fn test_parse_query_modifiers_case_insensitive_gx() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\GX");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.expanded);
		assert!(!mods.varset);
		assert_eq!(mods.prefix, None);
	}

	#[test]
	fn test_parse_query_modifiers_case_insensitive_gset() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\Gset prefix");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(!mods.expanded);
		assert!(mods.varset);
		assert_eq!(mods.prefix, Some("prefix".to_string()));
	}

	#[test]
	fn test_parse_query_modifiers_case_insensitive_gxset() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\GXSET myvar");
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.expanded);
		assert!(mods.varset);
		assert_eq!(mods.prefix, Some("myvar".to_string()));
	}

	#[test]
	fn test_parse_query_modifiers_gxset_prefix_no_space() {
		let (sql, mods) = parse_query_modifiers("SELECT * FROM users\\gxsetprefix");
		assert_eq!(sql, "SELECT * FROM users\\gxsetprefix");
		assert!(!mods.expanded);
		assert!(!mods.varset);
		assert_eq!(mods.prefix, None);
	}
}
