use rustyline::completion::Pair;

impl super::SqlCompleter {
	pub(super) fn complete_snippets(&self, text_before_cursor: &str) -> Option<Vec<Pair>> {
		// Check if we're completing snip subcommands after \snip
		if text_before_cursor.trim_start().starts_with(r"\snip ") {
			let after_snip = &text_before_cursor[6..];

			// If there's no space after what we've typed, we're still completing the subcommand
			if !after_snip.contains(' ') {
				let partial_cmd = after_snip.trim();
				let mut completions = Vec::new();

				// Offer snip subcommands
				for cmd in &["run", "save"] {
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
		}

		// Check if we're completing snippet names after \snip run or \snip save
		if text_before_cursor.trim_start().starts_with(r"\snip run ")
			|| text_before_cursor.trim_start().starts_with(r"\snip save ")
		{
			if let Some(repl_state_arc) = &self.repl_state {
				let repl_state = repl_state_arc.lock().unwrap();

				let cmd_start = if let Some(pos) = text_before_cursor.find(r"\snip run ") {
					pos + 10
				} else if let Some(pos) = text_before_cursor.find(r"\snip save ") {
					pos + 11
				} else {
					return Some(Vec::new());
				};

				let partial_name = text_before_cursor[cmd_start..].trim();

				let mut completions = Vec::new();

				// Try to get snippet names from all snippet directories
				for dir in &repl_state.snippets.dirs {
					if let Ok(entries) = std::fs::read_dir(dir) {
						for entry in entries.flatten() {
							if let Ok(file_name) = entry.file_name().into_string() {
								// Look for .sql files
								if file_name.ends_with(".sql") {
									let snippet_name = &file_name[..file_name.len() - 4];
									if snippet_name
										.to_lowercase()
										.starts_with(&partial_name.to_lowercase())
										&& !completions
											.iter()
											.any(|c: &Pair| c.display == snippet_name)
									{
										completions.push(Pair {
											display: snippet_name.to_string(),
											replacement: snippet_name.to_string(),
										});
									}
								}
							}
						}
					}
				}

				completions.sort_by(|a, b| a.display.cmp(&b.display));
				return Some(completions);
			}
		}

		None
	}
}

#[cfg(test)]
mod tests {
	use crate::completer::*;

	#[test]
	fn test_snip_subcommand_completion() {
		let completer = SqlCompleter::new(Theme::Dark);

		// Test completion of "run" subcommand
		let input = "\\snip r";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "run"));
		assert!(!completions.iter().any(|c| c.display == "save"));

		// Test completion of "save" subcommand
		let input = "\\snip s";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "save"));
		assert!(!completions.iter().any(|c| c.display == "run"));

		// Test completion of both subcommands when no prefix
		let input = "\\snip ";
		let completions = completer.find_completions(input, input.len());
		assert!(completions.iter().any(|c| c.display == "run"));
		assert!(completions.iter().any(|c| c.display == "save"));
	}

	#[test]
	fn test_snippet_run_completion() {
		use std::fs;
		use std::sync::{Arc, Mutex};
		use tempfile::TempDir;

		let temp_dir = TempDir::new().unwrap();
		let path = temp_dir.path();

		fs::create_dir_all(path).unwrap();
		fs::write(path.join("test1.sql"), "SELECT 1;").unwrap();
		fs::write(path.join("test2.sql"), "SELECT 2;").unwrap();
		fs::write(path.join("other.txt"), "not a snippet").unwrap();

		let snippets = crate::snippets::Snippets::with_savedir(path.to_path_buf());
		let repl_state = Arc::new(Mutex::new(crate::repl::ReplState {
			db_user: "test".to_string(),
			sys_user: "test".to_string(),
			expanded_mode: false,
			write_mode: false,
			ots: None,
			output_file: None,
			use_colours: true,
			vars: Default::default(),
			snippets,
			transaction_state: crate::repl::TransactionState::None,
			result_store: crate::result_store::ResultStore::new(),
		}));

		let mut completer = SqlCompleter::new(Theme::Dark);
		completer.repl_state = Some(Arc::clone(&repl_state));

		let input = "\\snip run t";
		let completions = completer.find_completions(input, input.len());

		assert!(completions.iter().any(|c| c.display == "test1"));
		assert!(completions.iter().any(|c| c.display == "test2"));
		assert!(!completions.iter().any(|c| c.display == "other"));
	}

	#[test]
	fn test_snippet_save_completion() {
		use std::fs;
		use std::sync::{Arc, Mutex};
		use tempfile::TempDir;

		let temp_dir = TempDir::new().unwrap();
		let path = temp_dir.path();

		fs::create_dir_all(path).unwrap();
		fs::write(path.join("snippet1.sql"), "SELECT 1;").unwrap();
		fs::write(path.join("snippet2.sql"), "SELECT 2;").unwrap();

		let snippets = crate::snippets::Snippets::with_savedir(path.to_path_buf());
		let repl_state = Arc::new(Mutex::new(crate::repl::ReplState {
			db_user: "test".to_string(),
			sys_user: "test".to_string(),
			expanded_mode: false,
			write_mode: false,
			ots: None,
			output_file: None,
			use_colours: true,
			vars: Default::default(),
			snippets,
			transaction_state: crate::repl::TransactionState::None,
			result_store: crate::result_store::ResultStore::new(),
		}));

		let mut completer = SqlCompleter::new(Theme::Dark);
		completer.repl_state = Some(Arc::clone(&repl_state));

		let input = "\\snip save snip";
		let completions = completer.find_completions(input, input.len());

		assert!(completions.iter().any(|c| c.display == "snippet1"));
		assert!(completions.iter().any(|c| c.display == "snippet2"));
	}

	#[test]
	fn test_snippet_completion_case_insensitive() {
		use std::fs;
		use std::sync::{Arc, Mutex};
		use tempfile::TempDir;

		let temp_dir = TempDir::new().unwrap();
		let path = temp_dir.path();

		fs::create_dir_all(path).unwrap();
		fs::write(path.join("TestSnippet.sql"), "SELECT 1;").unwrap();

		let snippets = crate::snippets::Snippets::with_savedir(path.to_path_buf());
		let repl_state = Arc::new(Mutex::new(crate::repl::ReplState {
			db_user: "test".to_string(),
			sys_user: "test".to_string(),
			expanded_mode: false,
			write_mode: false,
			ots: None,
			output_file: None,
			use_colours: true,
			vars: Default::default(),
			snippets,
			transaction_state: crate::repl::TransactionState::None,
			result_store: crate::result_store::ResultStore::new(),
		}));

		let mut completer = SqlCompleter::new(Theme::Dark);
		completer.repl_state = Some(Arc::clone(&repl_state));

		let input = "\\snip run test";
		let completions = completer.find_completions(input, input.len());

		assert!(completions.iter().any(|c| c.display == "TestSnippet"));
	}

	#[test]
	fn test_snippet_completion_no_duplicates() {
		use std::fs;
		use std::sync::{Arc, Mutex};
		use tempfile::TempDir;

		let temp_dir1 = TempDir::new().unwrap();
		let temp_dir2 = TempDir::new().unwrap();

		fs::create_dir_all(temp_dir1.path()).unwrap();
		fs::create_dir_all(temp_dir2.path()).unwrap();
		fs::write(temp_dir1.path().join("same.sql"), "SELECT 1;").unwrap();
		fs::write(temp_dir2.path().join("same.sql"), "SELECT 2;").unwrap();

		let mut snippets = crate::snippets::Snippets::with_savedir(temp_dir1.path().to_path_buf());
		snippets.dirs.push(temp_dir2.path().to_path_buf());

		let repl_state = Arc::new(Mutex::new(crate::repl::ReplState {
			db_user: "test".to_string(),
			sys_user: "test".to_string(),
			expanded_mode: false,
			write_mode: false,
			ots: None,
			output_file: None,
			use_colours: true,
			vars: Default::default(),
			snippets,
			transaction_state: crate::repl::TransactionState::None,
			result_store: crate::result_store::ResultStore::new(),
		}));

		let mut completer = SqlCompleter::new(Theme::Dark);
		completer.repl_state = Some(Arc::clone(&repl_state));

		let input = "\\snip run ";
		let completions = completer.find_completions(input, input.len());

		let same_count = completions.iter().filter(|c| c.display == "same").count();
		assert_eq!(same_count, 1);
	}
}
