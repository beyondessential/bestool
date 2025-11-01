use winnow::{Parser, ascii::space0, combinator::eof, error::ErrMode, token::literal};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal('e').parse_next(input)?;
	space0.parse_next(input)?;
	eof.parse_next(input)?;
	Ok(super::Metacommand::Edit)
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_edit() {
		let result = parse_metacommand("\\e").unwrap();
		assert_eq!(result, Some(Metacommand::Edit));
	}

	#[test]
	fn test_parse_metacommand_edit_with_whitespace() {
		let result = parse_metacommand("\\e   ").unwrap();
		assert_eq!(result, Some(Metacommand::Edit));
	}
}
