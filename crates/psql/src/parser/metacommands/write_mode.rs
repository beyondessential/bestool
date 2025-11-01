use winnow::{Parser, ascii::space0, combinator::eof, error::ErrMode, token::literal};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal('W').parse_next(input)?;
	space0.parse_next(input)?;
	eof.parse_next(input)?;
	Ok(super::Metacommand::WriteMode)
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_write_mode() {
		let result = parse_metacommand(r"\W").unwrap();
		assert_eq!(result, Some(Metacommand::WriteMode));
	}

	#[test]
	fn test_parse_metacommand_write_mode_with_whitespace() {
		let result = parse_metacommand(r"  \W  ").unwrap();
		assert_eq!(result, Some(Metacommand::WriteMode));
	}

	#[test]
	fn test_parse_metacommand_write_mode_with_trailing_text() {
		let result = parse_metacommand(r"\W some text").unwrap();
		assert_eq!(result, None);
	}
}
