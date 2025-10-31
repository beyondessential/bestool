use winnow::{
	ascii::{space0, space1},
	combinator::{eof, opt},
	error::ErrMode,
	token::literal,
	Parser,
};

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal('d').parse_next(input)?;

	// Parse modifiers: + for detail, ! for sameconn (in any order)
	let has_plus_first = opt(literal("+")).parse_next(input)?.is_some();
	let has_exclaim_first = opt(literal("!")).parse_next(input)?.is_some();
	let has_plus_second = opt(literal("+")).parse_next(input)?.is_some();
	let has_exclaim_second = opt(literal("!")).parse_next(input)?.is_some();

	let detail = has_plus_first || has_plus_second;
	let sameconn = has_exclaim_first || has_exclaim_second;

	// Must have a space before the item name
	space1.parse_next(input)?;

	// Parse the item name (everything until whitespace or end)
	let start = *input;
	let mut end_pos = 0;

	for (pos, ch) in start.char_indices() {
		if ch.is_whitespace() {
			break;
		}
		end_pos = pos + ch.len_utf8();
	}

	if end_pos == 0 {
		return Err(ErrMode::Backtrack(winnow::error::ContextError::new()));
	}

	let item = &start[..end_pos];
	*input = &start[end_pos..];

	space0.parse_next(input)?;
	eof.parse_next(input)?;

	Ok(super::Metacommand::Describe {
		item: item.to_string(),
		detail,
		sameconn,
	})
}

#[cfg(test)]
mod tests {
	use super::super::*;

	#[test]
	fn test_parse_describe() {
		let result = parse_metacommand("\\d users").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Describe {
				item: "users".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_describe_plus() {
		let result = parse_metacommand("\\d+ users").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Describe {
				item: "users".to_string(),
				detail: true,
				sameconn: false,
			})
		);
	}

	#[test]
	fn test_parse_describe_sameconn() {
		let result = parse_metacommand("\\d! users").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Describe {
				item: "users".to_string(),
				detail: false,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_describe_plus_sameconn() {
		let result = parse_metacommand("\\d+! users").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Describe {
				item: "users".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_describe_sameconn_plus() {
		let result = parse_metacommand("\\d!+ users").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Describe {
				item: "users".to_string(),
				detail: true,
				sameconn: true,
			})
		);
	}

	#[test]
	fn test_parse_describe_qualified() {
		let result = parse_metacommand("\\d public.users").unwrap();
		assert_eq!(
			result,
			Some(Metacommand::Describe {
				item: "public.users".to_string(),
				detail: false,
				sameconn: false,
			})
		);
	}
}
