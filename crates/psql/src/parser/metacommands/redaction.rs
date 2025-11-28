use winnow::{Parser, ascii::space0, combinator::eof, error::ErrMode, token::literal};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal('R').parse_next(input)?;
	space0.parse_next(input)?;
	eof.parse_next(input)?;
	Ok(super::Metacommand::ToggleRedaction)
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_toggle_redaction() {
		let result = parse_metacommand(r"\R").unwrap();
		assert_eq!(result, Some(Metacommand::ToggleRedaction));
	}

	#[test]
	fn test_parse_metacommand_toggle_redaction_with_whitespace() {
		let result = parse_metacommand(r"  \R  ").unwrap();
		assert_eq!(result, Some(Metacommand::ToggleRedaction));
	}

	#[test]
	fn test_parse_metacommand_toggle_redaction_with_text_after() {
		let result = parse_metacommand(r"\R on").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_lowercase_r_not_matched() {
		let result = parse_metacommand(r"\r").unwrap();
		assert_eq!(result, None);
	}
}
