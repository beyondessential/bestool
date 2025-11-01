use rustyline::completion::Pair;

impl super::SqlCompleter {
	pub(super) fn complete_result(&self, text_before_cursor: &str) -> Option<Vec<Pair>> {
		let trimmed = text_before_cursor.trim_start();

		if !trimmed.starts_with(r"\re ") {
			return None;
		}

		let after_re = &trimmed[4..];

		if !after_re.contains(' ') {
			let partial_cmd = after_re.trim();
			let mut completions = Vec::new();

			for cmd in &["format", "show", "list", "list+", "write"] {
				if cmd.starts_with(&partial_cmd.to_lowercase()) {
					completions.push(Pair {
						display: cmd.to_string(),
						replacement: cmd.to_string(),
					});
				}
			}

			if !completions.is_empty() {
				return Some(completions);
			}
		}

		if let Some(after_format) = trimmed.strip_prefix(r"\re format ") {
			let parts: Vec<&str> = after_format.split_whitespace().collect();

			let partial = if parts.is_empty() {
				""
			} else if parts.len() == 1 {
				if after_format.ends_with(' ') {
					""
				} else if parts[0].chars().all(|c| c.is_ascii_digit()) {
					return Some(Vec::new());
				} else {
					parts[0]
				}
			} else if parts.len() == 2 {
				if after_format.ends_with(' ') {
					return Some(Vec::new());
				}
				parts[1]
			} else {
				return Some(Vec::new());
			};

			let mut completions = Vec::new();
			for format in &[
				"table",
				"expanded",
				"json",
				"json-line",
				"json-array",
				"csv",
			] {
				if format.starts_with(&partial.to_lowercase()) {
					completions.push(Pair {
						display: format.to_string(),
						replacement: format.to_string(),
					});
				}
			}

			return Some(completions);
		}

		Some(Vec::new())
	}
}

#[cfg(test)]
mod tests {
	use crate::{completer::*, theme::Theme};

	#[test]
	fn test_re_subcommand_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re ";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "format"));
		assert!(completions.iter().any(|c| c.display == "show"));
		assert!(completions.iter().any(|c| c.display == "list"));
		assert!(completions.iter().any(|c| c.display == "list+"));
		assert!(completions.iter().any(|c| c.display == "write"));
	}

	#[test]
	fn test_re_format_subcommand_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re f";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "format"));
		assert!(!completions.iter().any(|c| c.display == "show"));
	}

	#[test]
	fn test_re_format_argument_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re format ";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "table"));
		assert!(completions.iter().any(|c| c.display == "expanded"));
		assert!(completions.iter().any(|c| c.display == "json"));
		assert!(completions.iter().any(|c| c.display == "json-line"));
		assert!(completions.iter().any(|c| c.display == "json-array"));
		assert!(completions.iter().any(|c| c.display == "csv"));
	}

	#[test]
	fn test_re_format_partial_argument_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re format js";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "json"));
		assert!(completions.iter().any(|c| c.display == "json-line"));
		assert!(completions.iter().any(|c| c.display == "json-array"));
		assert!(!completions.iter().any(|c| c.display == "table"));
		assert!(!completions.iter().any(|c| c.display == "csv"));
	}

	#[test]
	fn test_re_format_with_index() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re format 1 ";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "table"));
		assert!(completions.iter().any(|c| c.display == "json"));
	}

	#[test]
	fn test_re_format_with_index_partial() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re format 5 cs";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "csv"));
		assert!(!completions.iter().any(|c| c.display == "json"));
	}
}
