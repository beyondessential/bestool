use rustyline::completion::Pair;

impl super::SqlCompleter {
	pub(super) fn complete_result(&self, text_before_cursor: &str) -> Option<Vec<Pair>> {
		let trimmed = text_before_cursor.trim_start();

		if !trimmed.starts_with(r"\re ") {
			return None;
		}

		let after_re = &trimmed[4..];

		// Complete subcommand names
		if !after_re.contains(' ') {
			let partial_cmd = after_re.trim();
			let mut completions = Vec::new();

			for cmd in &["show", "list", "list+"] {
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

		// Handle \re show parameter completion
		if let Some(after_show) = trimmed.strip_prefix(r"\re show ") {
			return Some(self.complete_show_params(after_show));
		}

		Some(Vec::new())
	}

	fn complete_show_params(&self, after_show: &str) -> Vec<Pair> {
		let parts: Vec<&str> = after_show.split_whitespace().collect();

		// If we're at the end of a complete parameter, suggest all parameters
		if after_show.ends_with(' ') {
			return Self::get_all_param_completions(&parts);
		}

		// Get the last partial token
		let partial = parts.last().unwrap_or(&"");

		// If it contains '=', we're completing the value part
		if let Some((param_name, value_partial)) = partial.split_once('=') {
			return self.complete_param_value(param_name, value_partial);
		}

		// Otherwise, we're completing the parameter name
		Self::get_param_name_completions(partial, &parts)
	}

	fn get_all_param_completions(existing_parts: &[&str]) -> Vec<Pair> {
		let mut completions = Vec::new();
		let all_params = vec!["n=", "format=", "to=", "cols=", "limit=", "offset="];

		for param in all_params {
			let param_name = param.trim_end_matches('=');
			if !Self::is_param_already_used(param_name, existing_parts) {
				completions.push(Pair {
					display: param.to_string(),
					replacement: param.to_string(),
				});
			}
		}

		completions
	}

	fn get_param_name_completions(partial: &str, existing_parts: &[&str]) -> Vec<Pair> {
		let mut completions = Vec::new();
		let all_params = vec!["n=", "format=", "to=", "cols=", "limit=", "offset="];

		for param in all_params {
			if param.starts_with(partial) {
				let param_name = param.trim_end_matches('=');
				if !Self::is_param_already_used(param_name, existing_parts) {
					completions.push(Pair {
						display: param.to_string(),
						replacement: param.to_string(),
					});
				}
			}
		}

		completions
	}

	fn is_param_already_used(param_name: &str, existing_parts: &[&str]) -> bool {
		for part in existing_parts {
			if let Some((name, _)) = part.split_once('=')
				&& name == param_name
			{
				return true;
			}
		}
		false
	}

	fn complete_param_value(&self, param_name: &str, value_partial: &str) -> Vec<Pair> {
		match param_name {
			"format" => {
				let formats = vec!["table", "expanded", "json", "json-pretty", "csv"];
				let mut completions = Vec::new();
				for format in formats {
					if format.starts_with(value_partial) {
						completions.push(Pair {
							display: format.to_string(),
							replacement: format!("{}={}", param_name, format),
						});
					}
				}
				completions
			}
			"to" => {
				// Use path completion for file paths
				let path_completions = self.complete_file_path(value_partial);
				path_completions
					.into_iter()
					.map(|pair| Pair {
						display: pair.display,
						replacement: format!("{}={}", param_name, pair.replacement),
					})
					.collect()
			}
			// For other parameters (n, cols, limit, offset), we don't provide completions
			// as they require user-specific values
			_ => Vec::new(),
		}
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
		assert!(completions.iter().any(|c| c.display == "show"));
		assert!(completions.iter().any(|c| c.display == "list"));
		assert!(completions.iter().any(|c| c.display == "list+"));
		assert!(!completions.iter().any(|c| c.display == "format"));
		assert!(!completions.iter().any(|c| c.display == "write"));
	}

	#[test]
	fn test_re_show_subcommand_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re s";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "show"));
		assert!(!completions.iter().any(|c| c.display == "list"));
	}

	#[test]
	fn test_re_show_param_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re show ";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "n="));
		assert!(completions.iter().any(|c| c.display == "format="));
		assert!(completions.iter().any(|c| c.display == "to="));
		assert!(completions.iter().any(|c| c.display == "cols="));
		assert!(completions.iter().any(|c| c.display == "limit="));
		assert!(completions.iter().any(|c| c.display == "offset="));
	}

	#[test]
	fn test_re_show_partial_param_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re show f";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "format="));
		assert!(!completions.iter().any(|c| c.display == "n="));
		assert!(!completions.iter().any(|c| c.display == "limit="));
	}

	#[test]
	fn test_re_show_format_value_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re show format=";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "table"));
		assert!(completions.iter().any(|c| c.display == "expanded"));
		assert!(completions.iter().any(|c| c.display == "json"));
		assert!(completions.iter().any(|c| c.display == "json-pretty"));
		assert!(completions.iter().any(|c| c.display == "csv"));
	}

	#[test]
	fn test_re_show_format_partial_value_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re show format=js";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "json"));
		assert!(completions.iter().any(|c| c.display == "json-pretty"));
		assert!(!completions.iter().any(|c| c.display == "table"));
		assert!(!completions.iter().any(|c| c.display == "csv"));
	}

	#[test]
	fn test_re_show_multiple_params() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re show n=5 ";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "format="));
		assert!(completions.iter().any(|c| c.display == "to="));
		assert!(completions.iter().any(|c| c.display == "limit="));
		// n= should not appear again
		assert!(!completions.iter().any(|c| c.display == "n="));
	}

	#[test]
	fn test_re_show_no_duplicate_param_suggestions() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re show format=json limit=10 ";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "n="));
		assert!(completions.iter().any(|c| c.display == "to="));
		assert!(completions.iter().any(|c| c.display == "offset="));
		// format= and limit= should not appear again
		assert!(!completions.iter().any(|c| c.display == "format="));
		assert!(!completions.iter().any(|c| c.display == "limit="));
	}

	#[test]
	fn test_re_show_format_after_other_params() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re show n=3 format=";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "table"));
		assert!(completions.iter().any(|c| c.display == "json"));
	}

	#[test]
	fn test_re_show_no_value_completion_for_numeric_params() {
		let completer = SqlCompleter::new(Theme::Dark);

		// Should not provide completions for numeric parameters
		let input = r"\re show n=";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.is_empty());

		let input = r"\re show limit=";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.is_empty());
	}

	#[test]
	fn test_re_show_format_completion_replacement_includes_param_name() {
		let completer = SqlCompleter::new(Theme::Dark);

		let input = r"\re show format=c";
		let completions = completer.find_completions(input, input.len());

		// Find the csv completion
		let csv_completion = completions.iter().find(|c| c.display == "csv");
		assert!(csv_completion.is_some());

		// Verify the replacement includes "format="
		assert_eq!(csv_completion.unwrap().replacement, "format=csv");
	}

	#[test]
	fn test_re_show_to_path_completion() {
		use std::fs;
		use std::io::Write;

		// Create a temporary directory with test files
		let temp_dir = std::env::temp_dir().join("psql2_test_re_show_to");
		let _ = fs::remove_dir_all(&temp_dir);
		fs::create_dir_all(&temp_dir).unwrap();

		// Create test files
		let test_file1 = temp_dir.join("output1.json");
		let test_file2 = temp_dir.join("output2.csv");
		let test_dir = temp_dir.join("results");

		fs::File::create(&test_file1)
			.unwrap()
			.write_all(b"{}")
			.unwrap();
		fs::File::create(&test_file2)
			.unwrap()
			.write_all(b"data")
			.unwrap();
		fs::create_dir(&test_dir).unwrap();

		let completer = SqlCompleter::new(Theme::Dark);

		// Test completion with partial path
		let path_str = temp_dir.to_string_lossy();
		let input = format!(r"\re show to={}/output", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		// Files should be shown
		assert!(completions.iter().any(|c| c.display.starts_with("output")));
		// Replacement should include "to="
		let output_completion = completions
			.iter()
			.find(|c| c.display.starts_with("output1"));
		assert!(output_completion.is_some());
		assert!(output_completion.unwrap().replacement.starts_with("to="));

		// Test with directory listing
		let input = format!(r"\re show to={}/", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		// Directory should be listed with trailing slash
		assert!(completions.iter().any(|c| c.display == "results/"));

		// Cleanup
		let _ = fs::remove_dir_all(&temp_dir);
	}
}
