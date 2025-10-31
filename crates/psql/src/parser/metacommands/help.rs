use winnow::{
	ascii::space0,
	combinator::{alt, eof},
	error::ErrMode,
	token::literal,
	Parser,
};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	alt((literal('?'), literal("help"))).parse_next(input)?;
	space0.parse_next(input)?;
	eof.parse_next(input)?;
	Ok(super::Metacommand::Help)
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_help_question_mark() {
		let result = parse_metacommand(r"\?").unwrap();
		assert_eq!(result, Some(Metacommand::Help));
	}

	#[test]
	fn test_parse_metacommand_help_word() {
		let result = parse_metacommand(r"\help").unwrap();
		assert_eq!(result, Some(Metacommand::Help));
	}

	#[test]
	fn test_parse_metacommand_help_with_whitespace() {
		let result = parse_metacommand(r"  \?  ").unwrap();
		assert_eq!(result, Some(Metacommand::Help));
	}

	#[test]
	fn test_parse_metacommand_help_word_with_whitespace() {
		let result = parse_metacommand(r"  \help  ").unwrap();
		assert_eq!(result, Some(Metacommand::Help));
	}
}
