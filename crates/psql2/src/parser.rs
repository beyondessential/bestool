use std::collections::HashSet;

use miette::Result;
use winnow::{
	ascii::{space0, space1, Caseless},
	combinator::{alt, eof, opt, preceded},
	error::ErrMode,
	token::{literal, rest, take_till, take_while},
	Parser,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum QueryModifier {
	Expanded,
	Json,
	Verbatim,
	VarSet { prefix: Option<String> },
	Output { file_path: String },
}

pub(crate) type QueryModifiers = HashSet<QueryModifier>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DebugWhat {
	State,
	Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Metacommand {
	Quit,
	Expanded,
	WriteMode,
	Edit {
		content: Option<String>,
	},
	Include {
		file_path: String,
		vars: Vec<(String, String)>,
	},
	Output {
		file_path: Option<String>,
	},
	Debug {
		what: DebugWhat,
	},
	Help,
	SetVar {
		name: String,
		value: String,
	},
	UnsetVar {
		name: String,
	},
	LookupVar {
		pattern: Option<String>,
	},
	GetVar {
		name: String,
	},
	SnippetRun {
		name: String,
		vars: Vec<(String, String)>,
	},
	SnippetSave {
		name: String,
	},
}

pub(crate) fn parse_metacommand(input: &str) -> Result<Option<Metacommand>> {
	let input = input.trim();

	// Check if input starts with backslash
	if !input.starts_with('\\') {
		return Ok(None);
	}

	fn quit_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal('q').parse_next(input)?;
		space0.parse_next(input)?;
		eof.parse_next(input)?;
		Ok(Metacommand::Quit)
	}

	fn expanded_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal('x').parse_next(input)?;
		space0.parse_next(input)?;
		eof.parse_next(input)?;
		Ok(Metacommand::Expanded)
	}

	fn write_mode_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal('W').parse_next(input)?;
		space0.parse_next(input)?;
		eof.parse_next(input)?;
		Ok(Metacommand::WriteMode)
	}

	fn edit_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal('e').parse_next(input)?;
		let content = opt(preceded(space1, rest)).parse_next(input)?;
		Ok(Metacommand::Edit {
			content: content
				.map(|s: &str| s.trim().to_string())
				.filter(|s| !s.is_empty()),
		})
	}

	fn include_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal('i').parse_next(input)?;
		space1.parse_next(input)?;
		let file_path: &str = take_while(1.., |c: char| !c.is_whitespace()).parse_next(input)?;
		if file_path.is_empty() {
			return Err(ErrMode::Cut(winnow::error::ContextError::default()));
		}

		// Parse optional variable arguments
		let vars = parse_variable_args(input)?;

		Ok(Metacommand::Include {
			file_path: file_path.to_string(),
			vars,
		})
	}

	fn output_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal('o').parse_next(input)?;
		let file_path = opt(preceded(space1, rest)).parse_next(input)?;
		space0.parse_next(input)?;
		eof.parse_next(input)?;
		Ok(Metacommand::Output {
			file_path: file_path
				.map(|s: &str| s.trim().to_string())
				.filter(|s| !s.is_empty()),
		})
	}

	fn debug_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal("debug").parse_next(input)?;

		// Try to parse the argument
		let arg = opt(preceded(space1, rest)).parse_next(input)?;
		space0.parse_next(input)?;
		eof.parse_next(input)?;

		let what = if let Some(arg_str) = arg {
			let arg_trimmed = arg_str.trim();
			if arg_trimmed == "state" {
				DebugWhat::State
			} else {
				// Unknown argument, show help
				DebugWhat::Help
			}
		} else {
			// No argument, show help
			DebugWhat::Help
		};

		Ok(Metacommand::Debug { what })
	}

	fn snip_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal("snip").parse_next(input)?;

		let cmd: Option<&str> = opt(preceded(
			space1,
			take_while(1.., |c: char| !c.is_whitespace()),
		))
		.parse_next(input)?;

		let res = if let Some(cmd_str) = cmd {
			let name: Option<&str> = opt(preceded(
				space1,
				take_while(1.., |c: char| !c.is_whitespace()),
			))
			.parse_next(input)?;

			match (cmd_str, name) {
				("run", Some(name)) => {
					// Parse optional variable arguments
					let vars = parse_variable_args(input)?;
					Metacommand::SnippetRun {
						name: name.to_string(),
						vars,
					}
				}
				("save", Some(name)) => {
					space0.parse_next(input)?;
					eof.parse_next(input)?;
					Metacommand::SnippetSave {
						name: name.to_string(),
					}
				}
				_ => {
					space0.parse_next(input)?;
					eof.parse_next(input)?;
					Metacommand::Help
				}
			}
		} else {
			// No argument, show help
			space0.parse_next(input)?;
			eof.parse_next(input)?;
			Metacommand::Help
		};

		Ok(res)
	}

	fn parse_variable_args(
		input: &mut &str,
	) -> winnow::error::Result<Vec<(String, String)>, ErrMode<winnow::error::ContextError>> {
		let mut vars = Vec::new();

		loop {
			space0.parse_next(input)?;

			// Check if we're at EOF
			if opt(eof).parse_next(input)?.is_some() {
				break;
			}

			// Try to parse name=value
			let start_pos = input.len();
			let name_part: &str =
				take_while(1.., |c: char| c != '=' && !c.is_whitespace()).parse_next(input)?;

			if name_part.is_empty() {
				// Not a variable assignment, stop parsing
				*input = &input[start_pos..];
				break;
			}

			// Check for =
			if opt(literal('=')).parse_next(input)?.is_none() {
				// Not a variable assignment, rewind
				*input = &input[start_pos..];
				break;
			}

			// Parse value (everything until space or EOF)
			let value_part: &str =
				take_while(1.., |c: char| !c.is_whitespace()).parse_next(input)?;

			vars.push((name_part.to_string(), value_part.to_string()));
		}

		space0.parse_next(input)?;
		eof.parse_next(input)?;
		Ok(vars)
	}

	fn help_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		alt((literal('?'), literal("help"))).parse_next(input)?;
		space0.parse_next(input)?;
		eof.parse_next(input)?;
		Ok(Metacommand::Help)
	}

	fn set_var_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal("set").parse_next(input)?;
		space1.parse_next(input)?;
		let rest_str = rest.parse_next(input)?;
		let rest_trimmed = rest_str.trim();

		// Split on first whitespace to get name and value
		let parts: Vec<&str> = rest_trimmed
			.splitn(2, |c: char| c.is_whitespace())
			.collect();
		if parts.len() != 2 || parts[0].is_empty() || parts[1].trim().is_empty() {
			return Err(ErrMode::Cut(winnow::error::ContextError::default()));
		}

		Ok(Metacommand::SetVar {
			name: parts[0].to_string(),
			value: parts[1].trim().to_string(),
		})
	}

	fn unset_var_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal("unset").parse_next(input)?;
		space1.parse_next(input)?;
		let name = rest.parse_next(input)?;
		let name = name.trim();
		if name.is_empty() {
			return Err(ErrMode::Cut(winnow::error::ContextError::default()));
		}
		Ok(Metacommand::UnsetVar {
			name: name.to_string(),
		})
	}

	fn lookup_var_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal("vars").parse_next(input)?;
		let pattern = opt(preceded(space1, rest)).parse_next(input)?;
		space0.parse_next(input)?;
		eof.parse_next(input)?;
		Ok(Metacommand::LookupVar {
			pattern: pattern
				.map(|s: &str| s.trim().to_string())
				.filter(|s| !s.is_empty()),
		})
	}

	fn get_var_command(
		input: &mut &str,
	) -> winnow::error::Result<Metacommand, ErrMode<winnow::error::ContextError>> {
		literal('\\').parse_next(input)?;
		literal("get").parse_next(input)?;
		space1.parse_next(input)?;
		let name = rest.parse_next(input)?;
		let name = name.trim();
		if name.is_empty() {
			return Err(ErrMode::Cut(winnow::error::ContextError::default()));
		}
		Ok(Metacommand::GetVar {
			name: name.to_string(),
		})
	}

	let mut input_slice = input;
	if let Ok(cmd) = alt((
		quit_command,
		expanded_command,
		write_mode_command,
		edit_command,
		include_command,
		output_command,
		debug_command,
		help_command,
		snip_command,
		set_var_command,
		unset_var_command,
		lookup_var_command,
		get_var_command,
	))
	.parse_next(&mut input_slice)
	{
		Ok(Some(cmd))
	} else {
		Ok(None)
	}
}

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
			alt(('x', 'X')).map(|_| 'x'),
			alt(('j', 'J')).map(|_| 'j'),
			alt(('o', 'O')).map(|_| 'o'),
			alt(('v', 'V')).map(|_| 'v'),
		))
		.parse_next(input)
	}

	fn metacommand(
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
		let has_set = opt(literal(Caseless("set"))).parse_next(input)?.is_some();

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

	#[test]
	fn test_parse_query_modifiers_go() {
		let result = parse_query_modifiers("SELECT * FROM users\\go /tmp/output.txt").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Output {
			file_path: "/tmp/output.txt".to_string()
		}));
	}

	#[test]
	fn test_parse_query_modifiers_go_relative_path() {
		let result = parse_query_modifiers("SELECT 1\\go ./output/result.txt").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT 1");
		assert!(mods.contains(&QueryModifier::Output {
			file_path: "./output/result.txt".to_string()
		}));
	}

	#[test]
	fn test_parse_query_modifiers_go_uppercase() {
		let result = parse_query_modifiers("SELECT * FROM users\\gO /tmp/output.txt").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Output {
			file_path: "/tmp/output.txt".to_string()
		}));
	}

	#[test]
	fn test_parse_query_modifiers_gxo() {
		let result = parse_query_modifiers("SELECT * FROM users\\gxo /tmp/output.txt").unwrap();
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
		let result = parse_query_modifiers("SELECT * FROM users\\gjo /tmp/output.json").unwrap();
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
		let result = parse_query_modifiers("SELECT * FROM users\\gxjo /tmp/output.json").unwrap();
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
		let result = parse_query_modifiers("SELECT * FROM users\\go").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		// Should not contain Output modifier if no path provided
		assert!(!mods
			.iter()
			.any(|m| matches!(m, QueryModifier::Output { .. })));
	}

	#[test]
	fn test_parse_metacommand_quit() {
		let result = parse_metacommand("\\q").unwrap();
		assert_eq!(result, Some(Metacommand::Quit));
	}

	#[test]
	fn test_parse_metacommand_quit_with_whitespace() {
		let result = parse_metacommand("  \\q  ").unwrap();
		assert_eq!(result, Some(Metacommand::Quit));
	}

	#[test]
	fn test_parse_metacommand_expanded() {
		let result = parse_metacommand("\\x").unwrap();
		assert_eq!(result, Some(Metacommand::Expanded));
	}

	#[test]
	fn test_parse_metacommand_expanded_with_whitespace() {
		let result = parse_metacommand("  \\x  ").unwrap();
		assert_eq!(result, Some(Metacommand::Expanded));
	}

	#[test]
	fn test_parse_metacommand_not_metacommand() {
		let result = parse_metacommand("SELECT * FROM users").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_invalid_mushed() {
		let result = parse_metacommand("\\qx").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_with_trailing_text() {
		let result = parse_metacommand("\\q extra").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_query_modifier() {
		// \gx should not be parsed as metacommand
		let result = parse_metacommand("\\gx").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_unknown() {
		let result = parse_metacommand("\\z").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_empty_backslash() {
		let result = parse_metacommand("\\").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_quit_with_text_after() {
		let result = parse_metacommand("\\q quit now").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_expanded_with_text_after() {
		let result = parse_metacommand("\\x on").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_mixed_case() {
		let result = parse_metacommand("\\qX").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_no_backslash() {
		let result = parse_metacommand("q").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_empty_string() {
		let result = parse_metacommand("").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_write_mode() {
		let result = parse_metacommand("\\W").unwrap();
		assert_eq!(result, Some(Metacommand::WriteMode));
	}

	#[test]
	fn test_parse_metacommand_write_mode_with_whitespace() {
		let result = parse_metacommand("  \\W  ").unwrap();
		assert_eq!(result, Some(Metacommand::WriteMode));
	}

	#[test]
	fn test_parse_metacommand_write_mode_with_trailing_text() {
		let result = parse_metacommand("\\W some text").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_edit() {
		let result = parse_metacommand("\\e").unwrap();
		assert_eq!(result, Some(Metacommand::Edit { content: None }));
	}

	#[test]
	fn test_parse_metacommand_edit_with_content() {
		let result = parse_metacommand("\\e SELECT * FROM users").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Edit {
				content: Some("SELECT * FROM users".to_string())
			})
		);
	}

	#[test]
	fn test_parse_metacommand_edit_with_whitespace() {
		let result = parse_metacommand("\\e   ").unwrap();
		assert_eq!(result, Some(Metacommand::Edit { content: None }));
	}

	#[test]
	fn test_parse_metacommand_edit_with_content_and_whitespace() {
		let result = parse_metacommand("  \\e   SELECT 1  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Edit {
				content: Some("SELECT 1".to_string())
			})
		);
	}

	#[test]
	fn test_parse_metacommand_include() {
		let result = parse_metacommand("\\i /path/to/file.sql").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Include {
				file_path: "/path/to/file.sql".to_string(),
				vars: vec![]
			})
		);
	}

	#[test]
	fn test_parse_metacommand_include_with_whitespace() {
		let result = parse_metacommand("  \\i   /path/to/file.sql  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Include {
				file_path: "/path/to/file.sql".to_string(),
				vars: vec![]
			})
		);
	}

	#[test]
	fn test_parse_metacommand_include_relative_path() {
		let result = parse_metacommand("\\i ./queries/test.sql").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Include {
				file_path: "./queries/test.sql".to_string(),
				vars: vec![]
			})
		);
	}

	#[test]
	fn test_parse_metacommand_include_without_path() {
		let result = parse_metacommand("\\i").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_include_with_only_whitespace() {
		let result = parse_metacommand("\\i   ").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_output() {
		let result = parse_metacommand("\\o /path/to/output.txt").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Output {
				file_path: Some("/path/to/output.txt".to_string())
			})
		);
	}

	#[test]
	fn test_parse_metacommand_output_with_whitespace() {
		let result = parse_metacommand("  \\o   /path/to/output.txt  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Output {
				file_path: Some("/path/to/output.txt".to_string())
			})
		);
	}

	#[test]
	fn test_parse_metacommand_output_relative_path() {
		let result = parse_metacommand("\\o ./output/result.txt").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Output {
				file_path: Some("./output/result.txt".to_string())
			})
		);
	}

	#[test]
	fn test_parse_metacommand_output_without_path() {
		let result = parse_metacommand("\\o").unwrap();
		assert_eq!(result, Some(Metacommand::Output { file_path: None }));
	}

	#[test]
	fn test_parse_metacommand_output_with_only_whitespace() {
		let result = parse_metacommand("\\o   ").unwrap();
		assert_eq!(result, Some(Metacommand::Output { file_path: None }));
	}

	#[test]
	fn test_parse_metacommand_debug_state() {
		let result = parse_metacommand("\\debug state").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Debug {
				what: DebugWhat::State
			})
		);
	}

	#[test]
	fn test_parse_metacommand_debug_no_argument() {
		let result = parse_metacommand("\\debug").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Debug {
				what: DebugWhat::Help
			})
		);
	}

	#[test]
	fn test_parse_metacommand_debug_state_with_whitespace() {
		let result = parse_metacommand("  \\debug state  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Debug {
				what: DebugWhat::State
			})
		);
	}

	#[test]
	fn test_parse_metacommand_debug_state_with_extra_whitespace() {
		let result = parse_metacommand("\\debug  state").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Debug {
				what: DebugWhat::State
			})
		);
	}

	#[test]
	fn test_parse_metacommand_debug_unknown_argument() {
		let result = parse_metacommand("\\debug unknown").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Debug {
				what: DebugWhat::Help
			})
		);
	}

	#[test]
	fn test_parse_metacommand_help_question_mark() {
		let result = parse_metacommand("\\?").unwrap();
		assert_eq!(result, Some(Metacommand::Help));
	}

	#[test]
	fn test_parse_metacommand_help_word() {
		let result = parse_metacommand("\\help").unwrap();
		assert_eq!(result, Some(Metacommand::Help));
	}

	#[test]
	fn test_parse_metacommand_help_with_whitespace() {
		let result = parse_metacommand("  \\?  ").unwrap();
		assert_eq!(result, Some(Metacommand::Help));
	}

	#[test]
	fn test_parse_metacommand_help_word_with_whitespace() {
		let result = parse_metacommand("  \\help  ").unwrap();
		assert_eq!(result, Some(Metacommand::Help));
	}

	#[test]
	fn test_parse_metacommand_set_var() {
		let result = parse_metacommand("\\set myvar myvalue").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::SetVar {
				name: "myvar".to_string(),
				value: "myvalue".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_set_var_with_whitespace() {
		let result = parse_metacommand("  \\set  myvar  myvalue  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::SetVar {
				name: "myvar".to_string(),
				value: "myvalue".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_set_var_multiword_value() {
		let result = parse_metacommand("\\set myvar this is a long value").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::SetVar {
				name: "myvar".to_string(),
				value: "this is a long value".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_set_var_without_value() {
		let result = parse_metacommand("\\set myvar").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_set_var_without_name() {
		let result = parse_metacommand("\\set").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_unset_var() {
		let result = parse_metacommand("\\unset myvar").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::UnsetVar {
				name: "myvar".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_unset_var_with_whitespace() {
		let result = parse_metacommand("  \\unset  myvar  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::UnsetVar {
				name: "myvar".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_unset_var_without_name() {
		let result = parse_metacommand("\\unset").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_vars() {
		let result = parse_metacommand("\\vars").unwrap();
		assert_eq!(result, Some(Metacommand::LookupVar { pattern: None }));
	}

	#[test]
	fn test_parse_metacommand_vars_with_pattern() {
		let result = parse_metacommand("\\vars my*").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::LookupVar {
				pattern: Some("my*".to_string()),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_vars_with_whitespace() {
		let result = parse_metacommand("  \\vars  ").unwrap();
		assert_eq!(result, Some(Metacommand::LookupVar { pattern: None }));
	}

	#[test]
	fn test_parse_metacommand_vars_with_pattern_and_whitespace() {
		let result = parse_metacommand("  \\vars  pattern*  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::LookupVar {
				pattern: Some("pattern*".to_string()),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_get_var() {
		let result = parse_metacommand("\\get myvar").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::GetVar {
				name: "myvar".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_get_var_with_whitespace() {
		let result = parse_metacommand("  \\get  myvar  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::GetVar {
				name: "myvar".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_get_var_without_name() {
		let result = parse_metacommand("\\get").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_query_modifiers_gv() {
		let result = parse_query_modifiers("SELECT * FROM users\\gv").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Verbatim));
	}

	#[test]
	fn test_parse_query_modifiers_gvx() {
		let result = parse_query_modifiers("SELECT * FROM users\\gvx").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Verbatim));
		assert!(mods.contains(&QueryModifier::Expanded));
	}

	#[test]
	fn test_parse_query_modifiers_gxv() {
		let result = parse_query_modifiers("SELECT * FROM users\\gxv").unwrap();
		assert!(result.is_some());
		let (sql, mods) = result.unwrap();
		assert_eq!(sql, "SELECT * FROM users");
		assert!(mods.contains(&QueryModifier::Expanded));
		assert!(mods.contains(&QueryModifier::Verbatim));
	}

	#[test]
	fn test_parse_metacommand_snip_run() {
		let cmd = parse_metacommand("\\snip run mysnippet").unwrap();
		assert!(
			matches!(cmd, Some(Metacommand::SnippetRun { name, vars }) if name == "mysnippet" && vars.is_empty())
		);
	}

	#[test]
	fn test_parse_metacommand_snip_save() {
		let cmd = parse_metacommand("\\snip save mysnippet").unwrap();
		assert!(matches!(cmd, Some(Metacommand::SnippetSave { name }) if name == "mysnippet"));
	}

	#[test]
	fn test_parse_metacommand_snip_run_with_whitespace() {
		let cmd = parse_metacommand("\\snip run   mysnippet").unwrap();
		assert!(
			matches!(cmd, Some(Metacommand::SnippetRun { name, vars }) if name == "mysnippet" && vars.is_empty())
		);
	}

	#[test]
	fn test_parse_metacommand_snip_without_subcommand() {
		let cmd = parse_metacommand("\\snip").unwrap();
		assert!(matches!(cmd, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_metacommand_snip_invalid_subcommand() {
		let cmd = parse_metacommand("\\snip invalid name").unwrap();
		assert!(matches!(cmd, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_metacommand_snip_run_without_name() {
		let cmd = parse_metacommand("\\snip run").unwrap();
		assert!(matches!(cmd, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_metacommand_snip_run_with_vars() {
		let cmd = parse_metacommand("\\snip run mysnippet foo=bar baz=qux").unwrap();
		if let Some(Metacommand::SnippetRun { name, vars }) = cmd {
			assert_eq!(name, "mysnippet");
			assert_eq!(vars.len(), 2);
			assert_eq!(vars[0], ("foo".to_string(), "bar".to_string()));
			assert_eq!(vars[1], ("baz".to_string(), "qux".to_string()));
		} else {
			panic!("Expected SnippetRun");
		}
	}

	#[test]
	fn test_parse_metacommand_include_with_vars() {
		let cmd = parse_metacommand("\\i /path/to/file foo=bar").unwrap();
		if let Some(Metacommand::Include { file_path, vars }) = cmd {
			assert_eq!(file_path, "/path/to/file");
			assert_eq!(vars.len(), 1);
			assert_eq!(vars[0], ("foo".to_string(), "bar".to_string()));
		} else {
			panic!("Expected Include");
		}
	}

	#[test]
	fn test_parse_metacommand_include_with_multiple_vars() {
		let cmd = parse_metacommand("\\i file.sql a=1 b=2 c=3").unwrap();
		if let Some(Metacommand::Include { file_path, vars }) = cmd {
			assert_eq!(file_path, "file.sql");
			assert_eq!(vars.len(), 3);
			assert_eq!(vars[0], ("a".to_string(), "1".to_string()));
			assert_eq!(vars[1], ("b".to_string(), "2".to_string()));
			assert_eq!(vars[2], ("c".to_string(), "3".to_string()));
		} else {
			panic!("Expected Include");
		}
	}
}
