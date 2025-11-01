use winnow::{Parser, ascii::space0, combinator::eof, error::ErrMode, token::literal};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal('q').parse_next(input)?;
	space0.parse_next(input)?;
	eof.parse_next(input)?;
	Ok(super::Metacommand::Quit)
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_quit() {
		let result = parse_metacommand(r"\q").unwrap();
		assert_eq!(result, Some(Metacommand::Quit));
	}

	#[test]
	fn test_parse_metacommand_quit_with_whitespace() {
		let result = parse_metacommand(r"  \q  ").unwrap();
		assert_eq!(result, Some(Metacommand::Quit));
	}

	#[test]
	fn test_parse_metacommand_quit_with_text_after() {
		let result = parse_metacommand(r"\q quit now").unwrap();
		assert_eq!(result, None);
	}
}
