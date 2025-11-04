use rustyline::completion::Pair;

impl super::SqlCompleter {
	pub(super) fn complete_debug(&self, text_before_cursor: &str) -> Option<Vec<Pair>> {
		if !text_before_cursor.trim_start().starts_with(r"\debug ") {
			return None;
		}

		// Extract what's after \debug
		let debug_start = text_before_cursor.find(r"\debug ").unwrap() + 7;
		let partial_arg = &text_before_cursor[debug_start..].trim();

		let mut completions = Vec::new();

		// Offer "state" as completion
		if "state".starts_with(&partial_arg.to_lowercase()) {
			completions.push(Pair {
				display: "state".to_string(),
				replacement: "state".to_string(),
			});
		}

		// Offer "refresh-schema" as completion
		if "refresh-schema".starts_with(&partial_arg.to_lowercase()) {
			completions.push(Pair {
				display: "refresh-schema".to_string(),
				replacement: "refresh-schema".to_string(),
			});
		}

		Some(completions)
	}
}

#[cfg(test)]
mod tests {
	use crate::completer::*;

	#[test]
	fn test_debug_command_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions("\\", 1);
		assert!(completions.iter().any(|c| c.display == "\\debug"));
	}

	#[test]
	fn test_debug_state_argument_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		// Test with no argument
		let input = "\\debug ";
		let completions = completer.find_completions(input, input.len());
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "state"));
		assert!(completions.iter().any(|c| c.display == "refresh-schema"));

		// Test with partial argument for state
		let input = "\\debug st";
		let completions = completer.find_completions(input, input.len());
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "state"));

		// Test with partial argument for refresh-schema
		let input = "\\debug ref";
		let completions = completer.find_completions(input, input.len());
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "refresh-schema"));

		// Test with full argument should still match
		let input = "\\debug state";
		let completions = completer.find_completions(input, input.len());
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "state"));
	}
}
