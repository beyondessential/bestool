use winnow::{
	Parser,
	ascii::{space0, space1},
	combinator::{eof, opt, preceded},
	error::ErrMode,
	token::{literal, rest, take_while},
};

pub fn parse_set(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal("set").parse_next(input)?;
	space1.parse_next(input)?;
	let rest_str = rest.parse_next(input)?;
	let rest_trimmed = rest_str.trim();

	// Split on first whitespace to get name and value
	let parts: Vec<&str> = rest_trimmed
		.splitn(2, |c: char| c.is_whitespace())
		.collect();
	if parts.len() != 2 || parts[0].is_empty() || parts[1].trim().is_empty() {
		return Err(ErrMode::Cut(winnow::error::ContextError::default()));
	}

	Ok(super::Metacommand::SetVar {
		name: parts[0].to_string(),
		value: parts[1].trim().to_string(),
	})
}

pub fn parse_default(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal("default").parse_next(input)?;
	space1.parse_next(input)?;
	let rest_str = rest.parse_next(input)?;
	let rest_trimmed = rest_str.trim();

	// Split on first whitespace to get name and value
	let parts: Vec<&str> = rest_trimmed
		.splitn(2, |c: char| c.is_whitespace())
		.collect();
	if parts.len() != 2 || parts[0].is_empty() || parts[1].trim().is_empty() {
		return Err(ErrMode::Cut(winnow::error::ContextError::default()));
	}

	Ok(super::Metacommand::DefaultVar {
		name: parts[0].to_string(),
		value: parts[1].trim().to_string(),
	})
}

pub fn parse_unset(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal("unset").parse_next(input)?;
	space1.parse_next(input)?;
	let name = rest.parse_next(input)?;
	let name = name.trim();
	if name.is_empty() {
		return Err(ErrMode::Cut(winnow::error::ContextError::default()));
	}
	Ok(super::Metacommand::UnsetVar {
		name: name.to_string(),
	})
}

pub fn parse_lookup(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal("vars").parse_next(input)?;
	let pattern = opt(preceded(space1, rest)).parse_next(input)?;
	space0.parse_next(input)?;
	eof.parse_next(input)?;
	Ok(super::Metacommand::LookupVar {
		pattern: pattern
			.map(|s: &str| s.trim().to_string())
			.filter(|s| !s.is_empty()),
	})
}

pub fn parse_get(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal("get").parse_next(input)?;
	space1.parse_next(input)?;
	let name = rest.parse_next(input)?;
	let name = name.trim();
	if name.is_empty() {
		return Err(ErrMode::Cut(winnow::error::ContextError::default()));
	}
	Ok(super::Metacommand::GetVar {
		name: name.to_string(),
	})
}

pub fn parse_variable_args(
	input: &mut &str,
) -> winnow::error::Result<Vec<(String, String)>, ErrMode<winnow::error::ContextError>> {
	let mut vars = Vec::new();

	loop {
		space0.parse_next(input)?;

		// Check if we're at EOF
		if opt(eof).parse_next(input)?.is_some() {
			break;
		}

		// Try to parse name=value
		let start_pos = input.len();
		let name_part: &str =
			take_while(1.., |c: char| c != '=' && !c.is_whitespace()).parse_next(input)?;

		if name_part.is_empty() {
			// Not a variable assignment, stop parsing
			*input = &input[start_pos..];
			break;
		}

		// Check for =
		if opt(literal('=')).parse_next(input)?.is_none() {
			// Not a variable assignment, rewind
			*input = &input[start_pos..];
			break;
		}

		// Parse value (everything until space or EOF)
		let value_part: &str = take_while(1.., |c: char| !c.is_whitespace()).parse_next(input)?;

		vars.push((name_part.to_string(), value_part.to_string()));
	}

	space0.parse_next(input)?;
	eof.parse_next(input)?;
	Ok(vars)
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_metacommand_set_var() {
		let result = parse_metacommand(r"\set myvar myvalue").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::SetVar {
				name: "myvar".to_string(),
				value: "myvalue".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_default_var() {
		let result = parse_metacommand(r"\default myvar myvalue").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::DefaultVar {
				name: "myvar".to_string(),
				value: "myvalue".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_default_var_with_whitespace() {
		let result = parse_metacommand(r"  \default  myvar  myvalue  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::DefaultVar {
				name: "myvar".to_string(),
				value: "myvalue".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_default_var_multiword_value() {
		let result = parse_metacommand(r"\default myvar this is a long value").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::DefaultVar {
				name: "myvar".to_string(),
				value: "this is a long value".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_set_var_with_whitespace() {
		let result = parse_metacommand(r"  \set  myvar  myvalue  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::SetVar {
				name: "myvar".to_string(),
				value: "myvalue".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_set_var_multiword_value() {
		let result = parse_metacommand(r"\set myvar this is a long value").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::SetVar {
				name: "myvar".to_string(),
				value: "this is a long value".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_set_var_without_value() {
		let result = parse_metacommand(r"\set myvar").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_set_var_without_name() {
		let result = parse_metacommand(r"\set").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_unset_var() {
		let result = parse_metacommand(r"\unset myvar").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::UnsetVar {
				name: "myvar".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_unset_var_with_whitespace() {
		let result = parse_metacommand(r"  \unset  myvar  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::UnsetVar {
				name: "myvar".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_unset_var_without_name() {
		let result = parse_metacommand(r"\unset").unwrap();
		assert_eq!(result, None);
	}

	#[test]
	fn test_parse_metacommand_vars() {
		let result = parse_metacommand(r"\vars").unwrap();
		assert_eq!(result, Some(Metacommand::LookupVar { pattern: None }));
	}

	#[test]
	fn test_parse_metacommand_vars_with_pattern() {
		let result = parse_metacommand(r"\vars my*").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::LookupVar {
				pattern: Some("my*".to_string()),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_vars_with_whitespace() {
		let result = parse_metacommand(r"  \vars  ").unwrap();
		assert_eq!(result, Some(Metacommand::LookupVar { pattern: None }));
	}

	#[test]
	fn test_parse_metacommand_vars_with_pattern_and_whitespace() {
		let result = parse_metacommand(r"  \vars  pattern*  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::LookupVar {
				pattern: Some("pattern*".to_string()),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_get_var() {
		let result = parse_metacommand(r"\get myvar").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::GetVar {
				name: "myvar".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_get_var_with_whitespace() {
		let result = parse_metacommand(r"  \get  myvar  ").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::GetVar {
				name: "myvar".to_string(),
			})
		);
	}

	#[test]
	fn test_parse_metacommand_get_var_without_name() {
		let result = parse_metacommand(r"\get").unwrap();
		assert_eq!(result, None);
	}
}
