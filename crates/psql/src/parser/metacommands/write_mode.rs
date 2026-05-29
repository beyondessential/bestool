use winnow::{
	Parser,
	ascii::space0,
	error::ErrMode,
	token::{literal, rest},
};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal('W').parse_next(input)?;
	space0.parse_next(input)?;
	let trailing = rest.parse_next(input)?.trim();
	let ots = if trailing.is_empty() {
		None
	} else {
		Some(trailing.to_string())
	};
	Ok(super::Metacommand::WriteMode { ots })
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_write_mode() {
		let result = parse_metacommand(r"\W").unwrap();
		assert_eq!(result, Some(Metacommand::WriteMode { ots: None }));
	}

	#[test]
	fn test_parse_metacommand_write_mode_with_whitespace() {
		let result = parse_metacommand(r"  \W  ").unwrap();
		assert_eq!(result, Some(Metacommand::WriteMode { ots: None }));
	}

	#[test]
	fn test_parse_metacommand_write_mode_with_trailing_text() {
		let result = parse_metacommand(r"\W some text").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::WriteMode {
				ots: Some("some text".to_string())
			})
		);
	}

	#[test]
	fn test_parse_metacommand_write_mode_with_arg() {
		let result = parse_metacommand(r"\W bob").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::WriteMode {
				ots: Some("bob".to_string())
			})
		);
	}
}
