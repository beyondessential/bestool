use winnow::{
	Parser,
	ascii::{space0, space1},
	combinator::{eof, opt, preceded},
	error::ErrMode,
	token::{literal, rest},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DebugWhat {
	State,
	Help,
}

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
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

	Ok(super::Metacommand::Debug { what })
}

#[cfg(test)]
mod tests {
	use super::super::*;
	use super::*;

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
}
