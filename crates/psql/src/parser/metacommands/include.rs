use winnow::{
	Parser,
	ascii::space1,
	error::ErrMode,
	token::{literal, take_while},
};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal('i').parse_next(input)?;
	space1.parse_next(input)?;
	let file_path: &str = take_while(1.., |c: char| !c.is_whitespace()).parse_next(input)?;
	if file_path.is_empty() {
		return Err(ErrMode::Cut(winnow::error::ContextError::default()));
	}

	// Parse optional variable arguments
	let vars = super::vars::parse_variable_args(input)?;

	Ok(super::Metacommand::Include {
		file_path: file_path.to_string(),
		vars,
	})
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_include() {
		let result = parse_metacommand(r"\i /path/to/file.sql").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Include {
				file_path: "/path/to/file.sql".to_string(),
				vars: vec![]
			})
		);
	}

	#[test]
	fn test_parse_metacommand_include_with_whitespace() {
		let result = parse_metacommand(r"  \i   /path/to/file.sql  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Include {
				file_path: "/path/to/file.sql".to_string(),
				vars: vec![]
			})
		);
	}

	#[test]
	fn test_parse_metacommand_include_relative_path() {
		let result = parse_metacommand(r"\i ./queries/test.sql").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Include {
				file_path: "./queries/test.sql".to_string(),
				vars: vec![]
			})
		);
	}

	#[test]
	fn test_parse_metacommand_include_without_path() {
		let result = parse_metacommand(r"\i").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_include_with_only_whitespace() {
		let result = parse_metacommand(r"\i   ").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_include_with_vars() {
		let cmd = parse_metacommand(r"\i /path/to/file foo=bar").unwrap();
		if let Some(Metacommand::Include { file_path, vars }) = cmd {
			assert_eq!(file_path, "/path/to/file");
			assert_eq!(vars.len(), 1);
			assert_eq!(vars[0], ("foo".to_string(), "bar".to_string()));
		} else {
			panic!("Expected Include");
		}
	}

	#[test]
	fn test_parse_metacommand_include_with_multiple_vars() {
		let cmd = parse_metacommand(r"\i file.sql a=1 b=2 c=3").unwrap();
		if let Some(Metacommand::Include { file_path, vars }) = cmd {
			assert_eq!(file_path, "file.sql");
			assert_eq!(vars.len(), 3);
			assert_eq!(vars[0], ("a".to_string(), "1".to_string()));
			assert_eq!(vars[1], ("b".to_string(), "2".to_string()));
			assert_eq!(vars[2], ("c".to_string(), "3".to_string()));
		} else {
			panic!("Expected Include");
		}
	}
}
