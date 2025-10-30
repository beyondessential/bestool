use miette::Result;
use winnow::{combinator::alt, Parser};

pub(crate) use debug::DebugWhat;
pub use list::ListItem;

mod debug;
mod edit;
mod expanded;
mod help;
mod include;
mod list;
mod output;
mod quit;
mod snippets;
mod vars;
mod write_mode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Metacommand {
	Quit,
	Expanded,
	WriteMode,
	Edit,
	Include {
		file_path: String,
		vars: Vec<(String, String)>,
	},
	Output {
		file_path: Option<String>,
	},
	Debug {
		what: debug::DebugWhat,
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
	List {
		item: list::ListItem,
		pattern: String,
		detail: bool,
		sameconn: bool,
	},
}

pub(crate) fn parse_metacommand(input: &str) -> Result<Option<Metacommand>> {
	let input = input.trim();

	// Check if input starts with backslash
	if !input.starts_with('\\') {
		return Ok(None);
	}

	let mut input_slice = input;
	if let Ok(cmd) = alt((
		quit::parse,
		expanded::parse,
		write_mode::parse,
		edit::parse,
		include::parse,
		output::parse,
		debug::parse,
		help::parse,
		snippets::parse,
		vars::parse_set,
		vars::parse_unset,
		vars::parse_lookup,
		vars::parse_get,
		list::parse,
	))
	.parse_next(&mut input_slice)
	{
		Ok(Some(cmd))
	} else {
		Ok(None)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_metacommand_not_metacommand() {
		let result = parse_metacommand("SELECT * FROM users").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_invalid_mushed() {
		let result = parse_metacommand(r"\qx").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_with_trailing_text() {
		let result = parse_metacommand(r"\q extra").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_query_modifier() {
		// \gx should not be parsed as metacommand
		let result = parse_metacommand(r"\gx").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_unknown() {
		let result = parse_metacommand(r"\z").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_empty_backslash() {
		let result = parse_metacommand(r"\").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_mixed_case() {
		let result = parse_metacommand(r"\qX").unwrap();
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
}
