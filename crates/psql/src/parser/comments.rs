/// Strip SQL-style comments from a line
/// Returns the line with comments removed, or None if the line is only a comment
pub(crate) fn strip_comment(input: &str) -> Option<&str> {
	let mut in_single_quote = false;
	let mut in_double_quote = false;
	let mut prev_char = '\0';

	for (i, ch) in input.char_indices() {
		match ch {
			'\'' if !in_double_quote && prev_char != '\\' => {
				in_single_quote = !in_single_quote;
			}
			'"' if !in_single_quote && prev_char != '\\' => {
				in_double_quote = !in_double_quote;
			}
			'-' if !in_single_quote && !in_double_quote && prev_char == '-' => {
				// Found -- outside of quotes, strip from here
				let result = input[..i - 1].trim_end();
				if result.is_empty() {
					return None;
				}
				return Some(result);
			}
			_ => {}
		}
		prev_char = ch;
	}

	let trimmed = input.trim_end();
	if trimmed.is_empty() {
		None
	} else {
		Some(trimmed)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_strip_comment_no_comment() {
		assert_eq!(strip_comment("select 1 + 1"), Some("select 1 + 1"));
	}

	#[test]
	fn test_strip_comment_with_comment() {
		assert_eq!(strip_comment("select 1 + 1 -- foo"), Some("select 1 + 1"));
	}

	#[test]
	fn test_strip_comment_only_comment() {
		assert_eq!(strip_comment("-- foo"), None);
	}

	#[test]
	fn test_strip_comment_only_comment_with_spaces() {
		assert_eq!(strip_comment("  -- foo"), None);
	}

	#[test]
	fn test_strip_comment_in_single_quote() {
		assert_eq!(
			strip_comment("select '-- not a comment'"),
			Some("select '-- not a comment'")
		);
	}

	#[test]
	fn test_strip_comment_in_double_quote() {
		assert_eq!(
			strip_comment("select \"-- not a comment\""),
			Some("select \"-- not a comment\"")
		);
	}

	#[test]
	fn test_strip_comment_after_string() {
		assert_eq!(strip_comment("select 'foo' -- bar"), Some("select 'foo'"));
	}

	#[test]
	fn test_strip_comment_with_dash_in_string() {
		assert_eq!(
			strip_comment("select 'foo-bar' -- baz"),
			Some("select 'foo-bar'")
		);
	}

	#[test]
	fn test_strip_comment_metacommand() {
		assert_eq!(strip_comment("\\vars -- foo"), Some("\\vars"));
	}

	#[test]
	fn test_strip_comment_empty_string() {
		assert_eq!(strip_comment(""), None);
	}

	#[test]
	fn test_strip_comment_whitespace_only() {
		assert_eq!(strip_comment("   "), None);
	}
}
