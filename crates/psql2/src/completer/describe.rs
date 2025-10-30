use rustyline::completion::Pair;

impl super::SqlCompleter {
	pub(super) fn complete_describe(&self, text_before_cursor: &str) -> Option<Vec<Pair>> {
		let trimmed = text_before_cursor.trim_start();

		// Handle \d, \d+, \d!, \d+!, \d!+ with space after
		let after_describe = if let Some(after) = trimmed.strip_prefix(r"\d+ ") {
			Some(after)
		} else if let Some(after) = trimmed.strip_prefix(r"\d! ") {
			Some(after)
		} else if let Some(after) = trimmed.strip_prefix(r"\d+! ") {
			Some(after)
		} else if let Some(after) = trimmed.strip_prefix(r"\d!+ ") {
			Some(after)
		} else {
			trimmed.strip_prefix(r"\d ")
		};

		if let Some(partial) = after_describe {
			// Get the current word being typed
			let partial_lower = partial.to_lowercase();

			let mut completions = Vec::new();

			// Suggest tables, views, functions, and indexes from schema cache
			if let Some(cache) = &self.schema_cache {
				let cache = cache.read().unwrap();

				// Add tables
				for table in cache.all_tables() {
					if table.to_lowercase().starts_with(&partial_lower) {
						completions.push(Pair {
							display: table.clone(),
							replacement: table,
						});
					}
				}

				// Add views
				for view in cache.all_views() {
					if view.to_lowercase().starts_with(&partial_lower) {
						completions.push(Pair {
							display: view.clone(),
							replacement: view,
						});
					}
				}

				// Add functions
				for func in &cache.functions {
					if func.to_lowercase().starts_with(&partial_lower) {
						completions.push(Pair {
							display: func.clone(),
							replacement: func.clone(),
						});
					}
				}

				// Add indexes
				for index in cache.all_indexes() {
					if index.to_lowercase().starts_with(&partial_lower) {
						completions.push(Pair {
							display: index.clone(),
							replacement: index,
						});
					}
				}
			}

			// Sort and deduplicate
			completions.sort_by(|a, b| {
				let a_key = a.display.replace('_', "~");
				let b_key = b.display.replace('_', "~");
				a_key.cmp(&b_key)
			});
			completions.dedup_by(|a, b| a.display == b.display);

			return Some(completions);
		}

		None
	}
}

#[cfg(test)]
mod tests {
	use crate::{completer::*, theme::Theme};

	#[test]
	fn test_describe_command_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions("\\", 1);
		assert!(completions.iter().any(|c| c.display == "\\d"));
		assert!(completions.iter().any(|c| c.display == "\\d+"));
	}
}
