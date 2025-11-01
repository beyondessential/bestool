use std::path::Path;

use rustyline::completion::Pair;

impl super::SqlCompleter {
	pub(super) fn for_path_completion(text_before_cursor: &str) -> Option<&str> {
		if text_before_cursor.trim_start().starts_with(r"\i ")
			|| text_before_cursor.trim_start().starts_with(r"\o ")
		{
			let path_start = text_before_cursor
				.find(r"\o ")
				.or_else(|| text_before_cursor.find(r"\i "))
				.unwrap() + 3;
			let partial_path = &text_before_cursor[path_start..];
			return Some(partial_path);
		}

		if let Some(g_pos) = text_before_cursor.rfind(r"\g") {
			let after_g = &text_before_cursor[g_pos + 2..];
			// Check if it contains 'o' and is followed by a space
			if after_g.chars().any(|c| c == 'o')
				&& let Some(space_pos) = after_g.find(' ')
			{
				// Extract the file path after the space
				let partial_path = &after_g[space_pos + 1..];
				return Some(partial_path);
			}
		}

		None
	}

	pub(super) fn complete_file_path(&self, partial_path: &str) -> Vec<Pair> {
		let mut completions = Vec::new();

		// Determine the directory to search and the partial filename
		let (dir_path, partial_name) = if partial_path.is_empty() {
			(".", "")
		} else if partial_path.ends_with('/') || partial_path.ends_with('\\') {
			(partial_path, "")
		} else {
			let path = Path::new(partial_path);
			if let Some(parent) = path.parent() {
				let parent_str = if parent.as_os_str().is_empty() {
					"."
				} else {
					parent.to_str().unwrap_or(".")
				};
				let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
				(parent_str, name)
			} else {
				(".", partial_path)
			}
		};

		// Read directory entries
		if let Ok(entries) = std::fs::read_dir(dir_path) {
			for entry in entries.flatten() {
				if let Ok(file_name) = entry.file_name().into_string() {
					// Skip hidden files (starting with .) unless explicitly requested
					if file_name.starts_with('.') && !partial_name.starts_with('.') {
						continue;
					}

					// Filter by partial name (empty string matches everything)
					// Match case-insensitively
					if file_name
						.to_lowercase()
						.starts_with(&partial_name.to_lowercase())
					{
						let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

						// Build the replacement path
						let replacement = if dir_path == "." {
							if is_dir {
								format!("{}/", file_name)
							} else {
								file_name.clone()
							}
						} else {
							let mut path = Path::new(dir_path).join(&file_name);
							if is_dir {
								path = path.join("");
							}
							path.to_string_lossy().to_string()
						};

						let display = if is_dir {
							format!("{}/", file_name)
						} else {
							file_name
						};

						completions.push(Pair {
							display,
							replacement,
						});
					}
				}
			}
		}

		// Sort directories first, then files, both alphabetically
		completions.sort_by(|a, b| {
			let a_is_dir = a.display.ends_with('/');
			let b_is_dir = b.display.ends_with('/');
			match (a_is_dir, b_is_dir) {
				(true, false) => std::cmp::Ordering::Less,
				(false, true) => std::cmp::Ordering::Greater,
				_ => a.display.cmp(&b.display),
			}
		});

		completions
	}
}

#[cfg(test)]
mod tests {
	use crate::completer::*;

	#[test]
	fn test_include_command_file_path_completion() {
		use std::fs;
		use std::io::Write;

		// Create a temporary directory with test files
		let temp_dir = std::env::temp_dir().join("psql2_test_completion");
		let _ = fs::remove_dir_all(&temp_dir);
		fs::create_dir_all(&temp_dir).unwrap();

		// Create test files
		let test_file1 = temp_dir.join("test1.sql");
		let test_file2 = temp_dir.join("test2.sql");
		let test_dir = temp_dir.join("queries");

		fs::File::create(&test_file1)
			.unwrap()
			.write_all(b"SELECT 1;")
			.unwrap();
		fs::File::create(&test_file2)
			.unwrap()
			.write_all(b"SELECT 2;")
			.unwrap();
		fs::create_dir(&test_dir).unwrap();

		let completer = SqlCompleter::new(Theme::Dark);

		// Test completion with partial path
		let path_str = temp_dir.to_string_lossy();
		let input = format!(r"\i {}/test", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display.starts_with("test")));

		// Cleanup
		let _ = fs::remove_dir_all(&temp_dir);
	}

	#[test]
	fn test_include_command_directory_listing() {
		use std::fs;
		use std::io::Write;

		// Create a temporary directory with test files
		let temp_dir = std::env::temp_dir().join("psql2_test_dir_listing");
		let _ = fs::remove_dir_all(&temp_dir);
		fs::create_dir_all(&temp_dir).unwrap();

		// Create test files and directories
		let test_file1 = temp_dir.join("query1.sql");
		let test_file2 = temp_dir.join("query2.sql");
		let test_dir1 = temp_dir.join("queries");
		let test_dir2 = temp_dir.join("scripts");

		fs::File::create(&test_file1)
			.unwrap()
			.write_all(b"SELECT 1;")
			.unwrap();
		fs::File::create(&test_file2)
			.unwrap()
			.write_all(b"SELECT 2;")
			.unwrap();
		fs::create_dir(&test_dir1).unwrap();
		fs::create_dir(&test_dir2).unwrap();

		let completer = SqlCompleter::new(Theme::Dark);

		// Test completion with just the directory path (no partial filename)
		let path_str = temp_dir.to_string_lossy();
		let input = format!(r"\i {}/", path_str);
		let completions = completer.find_completions(&input, input.len());

		// Should list all files and directories
		assert!(!completions.is_empty());
		assert!(completions.len() >= 4);

		// Directories should come first (have trailing slash)
		let dir_count = completions
			.iter()
			.filter(|c| c.display.ends_with('/'))
			.count();
		assert!(dir_count >= 2);

		// Test with no path at all (current directory)
		// This should show files in the current working directory
		let input = r"\i ";
		let _completions = completer.find_completions(input, input.len());
		// Should have some completions (current dir likely has files)
		// We don't assert specific files since we don't control the working directory

		// Cleanup
		let _ = fs::remove_dir_all(&temp_dir);
	}

	#[test]
	fn test_include_command_case_insensitive_matching() {
		use std::fs;
		use std::io::Write;

		// Create a temporary directory with test files
		let temp_dir = std::env::temp_dir().join("psql2_test_case_insensitive");
		let _ = fs::remove_dir_all(&temp_dir);
		fs::create_dir_all(&temp_dir).unwrap();

		// Create test files with mixed case
		let test_file1 = temp_dir.join("Cargo.toml");
		let test_file2 = temp_dir.join("README.md");
		let test_dir = temp_dir.join("Scripts");

		fs::File::create(&test_file1)
			.unwrap()
			.write_all(b"[package]")
			.unwrap();
		fs::File::create(&test_file2)
			.unwrap()
			.write_all(b"# README")
			.unwrap();
		fs::create_dir(&test_dir).unwrap();

		let completer = SqlCompleter::new(Theme::Dark);

		// Test lowercase matching uppercase files
		let path_str = temp_dir.to_string_lossy();
		let input = format!(r"\i {}/cargo", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "Cargo.toml"));

		// Test uppercase matching mixed case
		let input = format!(r"\i {}/SCRIPTS", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "Scripts/"));

		// Test mixed case matching
		let input = format!(r"\i {}/ReAdMe", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "README.md"));

		// Cleanup
		let _ = fs::remove_dir_all(&temp_dir);
	}

	#[test]
	fn test_output_command_file_path_completion() {
		use std::fs;
		use std::io::Write;

		// Create a temporary directory with test files
		let temp_dir = std::env::temp_dir().join("psql2_test_output_completion");
		let _ = fs::remove_dir_all(&temp_dir);
		fs::create_dir_all(&temp_dir).unwrap();

		// Create test files
		let test_file1 = temp_dir.join("output1.txt");
		let test_file2 = temp_dir.join("output2.txt");

		fs::File::create(&test_file1)
			.unwrap()
			.write_all(b"test")
			.unwrap();
		fs::File::create(&test_file2)
			.unwrap()
			.write_all(b"test")
			.unwrap();

		let completer = SqlCompleter::new(Theme::Dark);

		// Test completion with partial path
		let path_str = temp_dir.to_string_lossy();
		let input = format!(r"\o {}/output", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display.starts_with("output")));

		// Cleanup
		let _ = fs::remove_dir_all(&temp_dir);
	}

	#[test]
	fn test_query_modifier_go_file_path_completion() {
		use std::fs;
		use std::io::Write;

		// Create a temporary directory with test files
		let temp_dir = std::env::temp_dir().join("psql2_test_go_completion");
		let _ = fs::remove_dir_all(&temp_dir);
		fs::create_dir_all(&temp_dir).unwrap();

		// Create test files
		let test_file1 = temp_dir.join("result1.txt");
		let test_file2 = temp_dir.join("result2.txt");

		fs::File::create(&test_file1)
			.unwrap()
			.write_all(b"test")
			.unwrap();
		fs::File::create(&test_file2)
			.unwrap()
			.write_all(b"test")
			.unwrap();

		let completer = SqlCompleter::new(Theme::Dark);

		// Test completion with \go
		let path_str = temp_dir.to_string_lossy();
		let input = format!("SELECT * FROM users\\go {}/result", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display.starts_with("result")));

		// Test completion with \gxo
		let input = format!("SELECT * FROM users\\gxo {}/result", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display.starts_with("result")));

		// Test completion with \gjo
		let input = format!("SELECT * FROM users\\gjo {}/result", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display.starts_with("result")));

		// Cleanup
		let _ = fs::remove_dir_all(&temp_dir);
	}
}
