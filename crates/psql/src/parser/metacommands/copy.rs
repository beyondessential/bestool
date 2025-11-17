use winnow::{Parser, error::ErrMode, token::literal};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal("copy").parse_next(input)?;
	// Accept and ignore any trailing text
	Ok(super::Metacommand::Copy)
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_copy() {
		let result = parse_metacommand(r"\copy").unwrap();
		assert_eq!(result, Some(Metacommand::Copy));
	}

	#[test]
	fn test_parse_metacommand_copy_with_whitespace() {
		let result = parse_metacommand(r"  \copy  ").unwrap();
		assert_eq!(result, Some(Metacommand::Copy));
	}

	#[test]
	fn test_parse_metacommand_copy_with_args() {
		let result = parse_metacommand(r"\copy (select from blah) with headers").unwrap();
		assert_eq!(result, Some(Metacommand::Copy));
	}

	#[test]
	fn test_parse_metacommand_copy_with_complex_args() {
		let result = parse_metacommand(r"\copy users to '/path/to/file.csv' csv header").unwrap();
		assert_eq!(result, Some(Metacommand::Copy));
	}
}
