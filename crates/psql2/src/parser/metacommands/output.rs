use winnow::{
	ascii::{space0, space1},
	combinator::{eof, opt, preceded},
	error::ErrMode,
	token::{literal, rest},
	Parser,
};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal('o').parse_next(input)?;
	let file_path = opt(preceded(space1, rest)).parse_next(input)?;
	space0.parse_next(input)?;
	eof.parse_next(input)?;
	Ok(super::Metacommand::Output {
		file_path: file_path
			.map(|s: &str| s.trim().to_string())
			.filter(|s| !s.is_empty()),
	})
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_output() {
		let result = parse_metacommand(r"\o /path/to/output.txt").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Output {
				file_path: Some("/path/to/output.txt".to_string())
			})
		);
	}

	#[test]
	fn test_parse_metacommand_output_with_whitespace() {
		let result = parse_metacommand(r"  \o   /path/to/output.txt  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Output {
				file_path: Some("/path/to/output.txt".to_string())
			})
		);
	}

	#[test]
	fn test_parse_metacommand_output_relative_path() {
		let result = parse_metacommand(r"\o ./output/result.txt").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Output {
				file_path: Some("./output/result.txt".to_string())
			})
		);
	}

	#[test]
	fn test_parse_metacommand_output_without_path() {
		let result = parse_metacommand(r"\o").unwrap();
		assert_eq!(result, Some(Metacommand::Output { file_path: None }));
	}

	#[test]
	fn test_parse_metacommand_output_with_only_whitespace() {
		let result = parse_metacommand(r"\o   ").unwrap();
		assert_eq!(result, Some(Metacommand::Output { file_path: None }));
	}
}
