use rustyline::completion::Pair;

impl super::SqlCompleter {
	pub(super) fn complete_list(&self, text_before_cursor: &str) -> Option<Vec<Pair>> {
		let trimmed = text_before_cursor.trim_start();

		// Handle \list command
		if let Some(after_list) = trimmed.strip_prefix(r"\list+ ") {
			return self.complete_list_args(after_list);
		}
		if let Some(after_list) = trimmed.strip_prefix(r"\list ") {
			return self.complete_list_args(after_list);
		}

		// Handle \dt command (pattern completion only)
		if trimmed.starts_with(r"\dt+ ") || trimmed.starts_with(r"\dt ") {
			// For \dt, we could complete schema names with wildcards
			// but for now, return empty to allow typing patterns freely
			return Some(Vec::new());
		}

		None
	}

	fn complete_list_args(&self, after_list: &str) -> Option<Vec<Pair>> {
		let parts: Vec<&str> = after_list.split_whitespace().collect();

		if parts.is_empty() {
			// Offer "table", "index", "function", and "view" as completions
			return Some(vec![
				Pair {
					display: "table".to_string(),
					replacement: "table".to_string(),
				},
				Pair {
					display: "index".to_string(),
					replacement: "index".to_string(),
				},
				Pair {
					display: "function".to_string(),
					replacement: "function".to_string(),
				},
				Pair {
					display: "view".to_string(),
					replacement: "view".to_string(),
				},
			]);
		}

		if parts.len() == 1 {
			let partial = parts[0];
			let mut completions = Vec::new();

			// Check if "table" matches
			if "table".starts_with(&partial.to_lowercase()) {
				completions.push(Pair {
					display: "table".to_string(),
					replacement: "table".to_string(),
				});
			}

			// Check if "index" matches
			if "index".starts_with(&partial.to_lowercase()) {
				completions.push(Pair {
					display: "index".to_string(),
					replacement: "index".to_string(),
				});
			}

			// Check if "function" matches
			if "function".starts_with(&partial.to_lowercase()) {
				completions.push(Pair {
					display: "function".to_string(),
					replacement: "function".to_string(),
				});
			}

			// Check if "view" matches
			if "view".starts_with(&partial.to_lowercase()) {
				completions.push(Pair {
					display: "view".to_string(),
					replacement: "view".to_string(),
				});
			}

			if !completions.is_empty() {
				return Some(completions);
			}
		}

		// For pattern completion after "table", "index", "function", or "view", we don't offer completions
		// to allow users to freely type patterns like "public.*" or "schema.table"
		Some(Vec::new())
	}
}

#[cfg(test)]
mod tests {
	use crate::{completer::*, theme::Theme};

	#[test]
	fn test_list_command_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions("\\", 1);
		assert!(completions.iter().any(|c| c.display == "\\list"));
		assert!(completions.iter().any(|c| c.display == "\\list+"));
		assert!(completions.iter().any(|c| c.display == "\\dt"));
		assert!(completions.iter().any(|c| c.display == "\\dt+"));
	}

	#[test]
	fn test_list_table_argument_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		// Test with no argument
		let input = "\\list ";
		let completions = completer.find_completions(input, input.len());
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "table"));
		assert!(completions.iter().any(|c| c.display == "index"));
		assert!(completions.iter().any(|c| c.display == "function"));
		assert!(completions.iter().any(|c| c.display == "view"));

		// Test with partial argument for table
		let input = "\\list ta";
		let completions = completer.find_completions(input, input.len());
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "table"));

		// Test with partial argument for index
		let input = "\\list in";
		let completions = completer.find_completions(input, input.len());
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "index"));

		// Test with full argument
		let input = "\\list table";
		let completions = completer.find_completions(input, input.len());
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "table"));
	}

	#[test]
	fn test_list_plus_table_argument_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = "\\list+ ";
		let completions = completer.find_completions(input, input.len());
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "table"));
		assert!(completions.iter().any(|c| c.display == "index"));
		assert!(completions.iter().any(|c| c.display == "function"));
		assert!(completions.iter().any(|c| c.display == "view"));
	}
}
