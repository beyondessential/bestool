use std::sync::{Arc, Mutex, RwLock};

use rustyline::completion::Pair;
use syntect::{highlighting::ThemeSet, parsing::SyntaxSet};

use crate::{repl::ReplState, schema_cache::SchemaCache, theme::Theme};

mod debug;
mod describe;
mod keywords;
mod list;
mod paths;
mod readline;
mod result;
mod snippets;
mod vars;

/// SQL keywords and psql commands for autocompletion
pub struct SqlCompleter {
	schema_cache: Option<Arc<RwLock<SchemaCache>>>,
	repl_state: Option<Arc<Mutex<ReplState>>>,
	syntax_set: SyntaxSet,
	theme_set: ThemeSet,
	theme: Theme,
}

impl SqlCompleter {
	pub fn new(theme: Theme) -> Self {
		Self {
			schema_cache: None,
			syntax_set: SyntaxSet::load_defaults_newlines(),
			theme_set: ThemeSet::load_defaults(),
			theme,
			repl_state: None,
		}
	}

	/// Set the schema cache for database-aware completion
	pub fn with_schema_cache(mut self, cache: Arc<RwLock<SchemaCache>>) -> Self {
		self.schema_cache = Some(cache);
		self
	}

	pub fn with_repl_state(mut self, repl_state: Arc<Mutex<ReplState>>) -> Self {
		self.repl_state = Some(repl_state);
		self
	}

	fn find_completions(&self, input: &str, pos: usize) -> Vec<Pair> {
		let text_before_cursor = &input[..pos];

		if let Some(partial_path) = Self::for_path_completion(text_before_cursor) {
			return self.complete_file_path(partial_path);
		}

		if let Some(completions) = self.complete_snippets(text_before_cursor) {
			return completions;
		}

		if let Some(completions) = self.complete_debug(text_before_cursor) {
			return completions;
		}

		if let Some(completions) = self.complete_vars(text_before_cursor) {
			return completions;
		}

		if let Some(completions) = self.complete_list(text_before_cursor) {
			return completions;
		}

		if let Some(completions) = self.complete_describe(text_before_cursor) {
			return completions;
		}

		if let Some(completions) = self.complete_result(text_before_cursor) {
			return completions;
		}

		let word_start = text_before_cursor
			.rfind(|c: char| c.is_whitespace() || c == '(' || c == ',' || c == ';')
			.map(|i| i + 1)
			.unwrap_or(0);

		let current_word = &text_before_cursor[word_start..];

		if current_word.is_empty() {
			return Vec::new();
		}

		let mut completions = Vec::new();

		if current_word.starts_with('\\') {
			for cmd in keywords::METACOMMAND {
				if cmd.to_lowercase().starts_with(&current_word.to_lowercase()) {
					completions.push(Pair {
						display: cmd.to_string(),
						replacement: cmd.to_string(),
					});
				}
			}
		} else {
			let input_lower = text_before_cursor.to_lowercase();

			if (input_lower.contains(" from ") || input_lower.starts_with("from "))
				&& let Some(cache) = &self.schema_cache {
					let cache = cache.read().unwrap();
					for table in cache.all_tables() {
						if table
							.to_lowercase()
							.starts_with(&current_word.to_lowercase())
						{
							completions.push(Pair {
								display: table.clone(),
								replacement: table,
							});
						}
					}
					for view in cache.all_views() {
						if view
							.to_lowercase()
							.starts_with(&current_word.to_lowercase())
						{
							completions.push(Pair {
								display: view.clone(),
								replacement: view,
							});
						}
					}
				}

			if let Some(cache) = &self.schema_cache {
				let cache = cache.read().unwrap();
				for schema in &cache.schemas {
					if schema
						.to_lowercase()
						.starts_with(&current_word.to_lowercase())
					{
						completions.push(Pair {
							display: schema.clone(),
							replacement: schema.clone(),
						});
					}
				}
			}

			let current_upper = current_word.to_uppercase();
			for keyword in keywords::SQL_KEYWORDS {
				if keyword.starts_with(&current_upper) {
					completions.push(Pair {
						display: keyword.to_string(),
						replacement: keyword.to_string(),
					});
				}
			}

			if let Some(cache) = &self.schema_cache {
				let cache = cache.read().unwrap();
				for table in cache.all_tables() {
					if table
						.to_lowercase()
						.starts_with(&current_word.to_lowercase())
						&& !completions.iter().any(|c| c.display == table)
					{
						completions.push(Pair {
							display: table.clone(),
							replacement: table,
						});
					}
				}
			}

			if let Some(cache) = &self.schema_cache {
				let cache = cache.read().unwrap();
				for func in &cache.functions {
					if func
						.to_lowercase()
						.starts_with(&current_word.to_lowercase())
					{
						completions.push(Pair {
							display: func.clone(),
							replacement: func.clone(),
						});
					}
				}
			}
		}

		completions.sort_by(|a, b| {
			let a_key = a.display.replace('_', "~");
			let b_key = b.display.replace('_', "~");
			a_key.cmp(&b_key)
		});
		completions.dedup_by(|a, b| a.display == b.display);
		completions
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_sql_keyword_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions("SEL", 3);
		assert!(completions.iter().any(|c| c.display == "SELECT"));
	}

	#[test]
	fn test_case_insensitive_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions("select", 6);
		assert!(completions.iter().any(|c| c.display == "SELECT"));
	}

	#[test]
	fn test_mid_query_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions("SELECT * FRO", 12);
		assert!(completions.iter().any(|c| c.display == "FROM"));
	}

	#[test]
	fn test_include_command_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions(r"\", 1);
		assert!(completions.iter().any(|c| c.display == r"\i"));
	}

	#[test]
	fn test_output_command_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions(r"\", 1);
		assert!(completions.iter().any(|c| c.display == r"\o"));
	}

	#[test]
	fn test_help_command_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions(r"\", 1);
		assert!(completions.iter().any(|c| c.display == r"\?"));
		assert!(completions.iter().any(|c| c.display == r"\help"));
	}

	#[test]
	fn test_help_question_mark_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions(r"\?", 2);
		assert!(completions.iter().any(|c| c.display == r"\?"));
	}

	#[test]
	fn test_help_word_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions(r"\hel", 4);
		assert!(completions.iter().any(|c| c.display == r"\help"));
	}
}
