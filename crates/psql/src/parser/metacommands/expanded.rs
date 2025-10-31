use winnow::{ascii::space0, combinator::eof, error::ErrMode, token::literal, Parser};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal('x').parse_next(input)?;
	space0.parse_next(input)?;
	eof.parse_next(input)?;
	Ok(super::Metacommand::Expanded)
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_expanded() {
		let result = parse_metacommand(r"\x").unwrap();
		assert_eq!(result, Some(Metacommand::Expanded));
	}

	#[test]
	fn test_parse_metacommand_expanded_with_whitespace() {
		let result = parse_metacommand(r"  \x  ").unwrap();
		assert_eq!(result, Some(Metacommand::Expanded));
	}

	#[test]
	fn test_parse_metacommand_expanded_with_text_after() {
		let result = parse_metacommand(r"\x on").unwrap();
		assert_eq!(result, None);
	}
}
