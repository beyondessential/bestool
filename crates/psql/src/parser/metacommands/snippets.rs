use winnow::{
	Parser,
	ascii::{space0, space1},
	combinator::{eof, opt, preceded},
	error::ErrMode,
	token::{literal, take_while},
};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
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
				let vars = super::vars::parse_variable_args(input)?;
				super::Metacommand::SnippetRun {
					name: name.to_string(),
					vars,
				}
			}
			("save", Some(name)) => {
				space0.parse_next(input)?;
				eof.parse_next(input)?;
				super::Metacommand::SnippetSave {
					name: name.to_string(),
				}
			}
			_ => {
				space0.parse_next(input)?;
				eof.parse_next(input)?;
				super::Metacommand::Help
			}
		}
	} else {
		// No argument, show help
		space0.parse_next(input)?;
		eof.parse_next(input)?;
		super::Metacommand::Help
	};

	Ok(res)
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_snip_run() {
		let cmd = parse_metacommand(r"\snip run mysnippet").unwrap();
		assert!(
			matches!(cmd, Some(Metacommand::SnippetRun { name, vars }) if name == "mysnippet" && vars.is_empty())
		);
	}

	#[test]
	fn test_parse_metacommand_snip_save() {
		let cmd = parse_metacommand(r"\snip save mysnippet").unwrap();
		assert!(matches!(cmd, Some(Metacommand::SnippetSave { name }) if name == "mysnippet"));
	}

	#[test]
	fn test_parse_metacommand_snip_run_with_whitespace() {
		let cmd = parse_metacommand(r"\snip run   mysnippet").unwrap();
		assert!(
			matches!(cmd, Some(Metacommand::SnippetRun { name, vars }) if name == "mysnippet" && vars.is_empty())
		);
	}

	#[test]
	fn test_parse_metacommand_snip_without_subcommand() {
		let cmd = parse_metacommand(r"\snip").unwrap();
		assert!(matches!(cmd, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_metacommand_snip_invalid_subcommand() {
		let cmd = parse_metacommand(r"\snip invalid name").unwrap();
		assert!(matches!(cmd, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_metacommand_snip_run_without_name() {
		let cmd = parse_metacommand(r"\snip run").unwrap();
		assert!(matches!(cmd, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_metacommand_snip_run_with_vars() {
		let cmd = parse_metacommand(r"\snip run mysnippet foo=bar baz=qux").unwrap();
		if let Some(Metacommand::SnippetRun { name, vars }) = cmd {
			assert_eq!(name, "mysnippet");
			assert_eq!(vars.len(), 2);
			assert_eq!(vars[0], ("foo".to_string(), "bar".to_string()));
			assert_eq!(vars[1], ("baz".to_string(), "qux".to_string()));
		} else {
			panic!("Expected SnippetRun");
		}
	}
}
