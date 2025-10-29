use std::borrow::Cow;
use std::path::Path;
use std::sync::{Arc, RwLock};

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, Helper};
use syntect::{
	easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet,
	util::as_24_bit_terminal_escaped,
};

use crate::highlighter::Theme;
use crate::schema_cache::SchemaCache;

/// SQL keywords and psql commands for autocompletion
pub struct SqlCompleter {
	keywords: Vec<&'static str>,
	psql_commands: Vec<&'static str>,
	schema_cache: Option<Arc<RwLock<SchemaCache>>>,
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
			keywords: vec![
				"SELECT",
				"FROM",
				"WHERE",
				"AND",
				"OR",
				"NOT",
				"IN",
				"EXISTS",
				"INSERT",
				"INTO",
				"VALUES",
				"UPDATE",
				"SET",
				"DELETE",
				"CREATE",
				"ALTER",
				"DROP",
				"TRUNCATE",
				"TABLE",
				"INDEX",
				"VIEW",
				"SEQUENCE",
				"SCHEMA",
				"DATABASE",
				"JOIN",
				"INNER",
				"LEFT",
				"RIGHT",
				"FULL",
				"OUTER",
				"CROSS",
				"ON",
				"USING",
				"AS",
				"DISTINCT",
				"ALL",
				"ANY",
				"SOME",
				"ORDER",
				"BY",
				"GROUP",
				"HAVING",
				"LIMIT",
				"OFFSET",
				"UNION",
				"INTERSECT",
				"EXCEPT",
				"CASE",
				"WHEN",
				"THEN",
				"ELSE",
				"END",
				"NULL",
				"IS",
				"LIKE",
				"ILIKE",
				"SIMILAR",
				"TO",
				"BETWEEN",
				"ASC",
				"DESC",
				"NULLS",
				"FIRST",
				"LAST",
				"WITH",
				"RECURSIVE",
				"CTE",
				"WINDOW",
				"OVER",
				"PARTITION",
				"ROWS",
				"RANGE",
				"BEGIN",
				"COMMIT",
				"ROLLBACK",
				"SAVEPOINT",
				"RELEASE",
				"TRANSACTION",
				"ISOLATION",
				"LEVEL",
				"READ",
				"WRITE",
				"SERIALIZABLE",
				"REPEATABLE",
				"UNCOMMITTED",
				"COMMITTED",
				"GRANT",
				"REVOKE",
				"PRIVILEGES",
				"PUBLIC",
				"PRIMARY",
				"KEY",
				"FOREIGN",
				"REFERENCES",
				"UNIQUE",
				"CHECK",
				"DEFAULT",
				"CONSTRAINT",
				"CASCADE",
				"RESTRICT",
				"EXPLAIN",
				"ANALYZE",
				"VERBOSE",
				"COPY",
				"RETURNING",
				"INTEGER",
				"INT",
				"BIGINT",
				"SMALLINT",
				"SERIAL",
				"BIGSERIAL",
				"NUMERIC",
				"DECIMAL",
				"REAL",
				"DOUBLE",
				"PRECISION",
				"VARCHAR",
				"CHAR",
				"TEXT",
				"BYTEA",
				"TIMESTAMP",
				"TIMESTAMPTZ",
				"DATE",
				"TIME",
				"TIMETZ",
				"INTERVAL",
				"BOOLEAN",
				"BOOL",
				"TRUE",
				"FALSE",
				"UUID",
				"JSON",
				"JSONB",
				"ARRAY",
				"COUNT",
				"SUM",
				"AVG",
				"MIN",
				"MAX",
				"COALESCE",
				"NULLIF",
				"GREATEST",
				"LEAST",
				"CAST",
				"CONVERT",
				"CURRENT_TIMESTAMP",
				"CURRENT_DATE",
				"CURRENT_TIME",
				"NOW",
				"AGE",
				"EXTRACT",
				"CONCAT",
				"LENGTH",
				"SUBSTRING",
				"TRIM",
				"UPPER",
				"LOWER",
				"ARRAY_AGG",
				"STRING_AGG",
				"JSON_AGG",
				"JSONB_AGG",
				"ROW_NUMBER",
				"RANK",
				"DENSE_RANK",
				"LAG",
				"LEAD",
				"ISNULL",
				"NOTNULL",
				"TABLESAMPLE",
				"LATERAL",
				"GENERATE_SERIES",
				"UNNEST",
				"VACUUM",
				"ANALYZE",
				"REINDEX",
				"CLUSTER",
			],
			psql_commands: vec![
				// "\\?",
				// "\\h",
				"\\q",
				// "\\c",
				// "\\d",
				// "\\dt",
				// "\\di",
				// "\\dv",
				// "\\ds",
				// "\\df",
				// "\\dT",
				// "\\du",
				// "\\dn",
				// "\\dp",
				// "\\l",
				// "\\z",
				// "\\d+",
				// "\\dt+",
				// "\\di+",
				// "\\dv+",
				// "\\ds+",
				// "\\df+",
				// "\\dT+",
				// "\\du+",
				// "\\dn+",
				// "\\dp+",
				// "\\l+",
				// "\\da",
				// "\\db",
				// "\\dc",
				// "\\dC",
				// "\\dd",
				// "\\dD",
				// "\\ddp",
				// "\\dE",
				// "\\des",
				// "\\det",
				// "\\deu",
				// "\\dew",
				// "\\dF",
				// "\\dFd",
				// "\\dFp",
				// "\\dFt",
				// "\\dg",
				// "\\dL",
				// "\\dm",
				// "\\do",
				// "\\dO",
				// "\\drds",
				// "\\dRs",
				// "\\dRp",
				// "\\dy",
				"\\e",
				// "\\ef",
				// "\\ev",
				// "\\edit",
				// "\\echo",
				// "\\qecho",
				// "\\warn",
				"\\i", // "\\ir",
				// "\\include",
				// "\\include_relative",
				"\\o",
				// "\\out",
				// "\\p",
				// "\\print",
				// "\\r",
				// "\\reset",
				// "\\s",
				// "\\history",
				// "\\w",
				// "\\write",
				"\\x",
				// "\\expanded",
				"\\g",
				// "\\go",
				// "\\gx",
				// "\\gexec",
				// "\\gset",
				// "\\watch",
				// "\\timing",
				// "\\t",
				// "\\tuples_only",
				// "\\a",
				// "\\aligned",
				// "\\C",
				// "\\caption",
				// "\\f",
				// "\\fieldsep",
				// "\\fieldsep_zero",
				// "\\H",
				// "\\html",
				// "\\T",
				// "\\tableattr",
				// "\\pset",
				// "\\P",
				// "\\pager",
				// "\\encoding",
				// "\\password",
				// "\\cd",
				// "\\setenv",
				// "\\!",
				// "\\shell",
				// "\\copy",
				// "\\crosstabview",
				// "\\errverbose",
				// "\\gdesc",
				// "\\set",
				// "\\unset",
				// "\\prompt",
				// "\\if",
				// "\\elif",
				// "\\else",
				// "\\endif",
				// "\\lo_import",
				// "\\lo_export",
				// "\\lo_list",
				// "\\lo_unlink",
				// "\\conninfo",
				// "\\connect",
			],
		}
	}

	/// Set the schema cache for database-aware completion
	pub fn with_schema_cache(mut self, cache: Arc<RwLock<SchemaCache>>) -> Self {
		self.schema_cache = Some(cache);
		self
	}

	fn find_completions(&self, input: &str, pos: usize) -> Vec<Pair> {
		let text_before_cursor = &input[..pos];

		// Check if we're completing a file path after \i command
		// Do this check before checking for empty current_word so we can show directory listings
		if text_before_cursor.trim_start().starts_with("\\i ") {
			// Extract the file path being typed
			let path_start = text_before_cursor.find("\\i ").unwrap() + 3;
			let partial_path = &text_before_cursor[path_start..];

			return self.complete_file_path(partial_path);
		}

		// Check if we're completing a file path after \o command
		if text_before_cursor.trim_start().starts_with("\\o ") {
			// Extract the file path being typed
			let path_start = text_before_cursor.find("\\o ").unwrap() + 3;
			let partial_path = &text_before_cursor[path_start..];

			return self.complete_file_path(partial_path);
		}

		// Check if we're completing a file path after \g...o query modifier (e.g. \go, \gxo, \gjo, \gxjo)
		// This is more complex because we need to find \g followed by optional modifiers and then 'o'
		if let Some(g_pos) = text_before_cursor.rfind("\\g") {
			let after_g = &text_before_cursor[g_pos + 2..];
			// Check if it contains 'o' or 'O' and is followed by a space
			if after_g.chars().any(|c| c == 'o' || c == 'O') {
				if let Some(space_pos) = after_g.find(' ') {
					// Extract the file path after the space
					let partial_path = &after_g[space_pos + 1..];
					return self.complete_file_path(partial_path);
				}
			}
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
			for cmd in &self.psql_commands {
				if cmd.to_lowercase().starts_with(&current_word.to_lowercase()) {
					completions.push(Pair {
						display: cmd.to_string(),
						replacement: cmd.to_string(),
					});
				}
			}
		} else {
			let input_lower = text_before_cursor.to_lowercase();

			if input_lower.contains(" from ") || input_lower.starts_with("from ") {
				if let Some(cache) = &self.schema_cache {
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
			for keyword in &self.keywords {
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

	fn complete_file_path(&self, partial_path: &str) -> Vec<Pair> {
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

impl Completer for SqlCompleter {
	type Candidate = Pair;

	fn complete(
		&self,
		line: &str,
		pos: usize,
		_ctx: &Context<'_>,
	) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
		let completions = self.find_completions(line, pos);

		let text_before_cursor = &line[..pos];
		let word_start = text_before_cursor
			.rfind(|c: char| c.is_whitespace() || c == '(' || c == ',' || c == ';')
			.map(|i| i + 1)
			.unwrap_or(0);

		Ok((word_start, completions))
	}
}

impl Hinter for SqlCompleter {
	type Hint = String;

	fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
		None
	}
}

impl Highlighter for SqlCompleter {
	fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
		let syntax = self
			.syntax_set
			.find_syntax_by_extension("sql")
			.unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

		let resolved_theme = self.theme.resolve();
		let theme_name = match resolved_theme {
			Theme::Light => "base16-ocean.light",
			Theme::Dark => "base16-ocean.dark",
			Theme::Auto => unreachable!("resolve() always returns Light or Dark"),
		};

		let theme = &self.theme_set.themes[theme_name];

		let mut highlighter = HighlightLines::new(syntax, theme);
		match highlighter.highlight_line(line, &self.syntax_set) {
			Ok(ranges) => {
				let mut escaped = as_24_bit_terminal_escaped(&ranges[..], false);
				escaped.push_str("\x1b[0m");
				Cow::Owned(escaped)
			}
			Err(_) => Cow::Borrowed(line),
		}
	}

	fn highlight_char(&self, _line: &str, _pos: usize, _forced: CmdKind) -> bool {
		true
	}
}

impl Validator for SqlCompleter {
	fn validate(&self, _ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
		Ok(ValidationResult::Valid(None))
	}
}

impl Helper for SqlCompleter {}

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
	#[ignore = "psql meta-commands not yet supported"]
	fn test_psql_command_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions("\\d", 2);
		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "\\dt"));
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
		let completions = completer.find_completions("\\", 1);
		assert!(completions.iter().any(|c| c.display == "\\i"));
	}

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
		let input = format!("\\i {}/test", path_str);
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
		let input = format!("\\i {}/", path_str);
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
		let input = "\\i ";
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
		let input = format!("\\i {}/cargo", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "Cargo.toml"));

		// Test uppercase matching mixed case
		let input = format!("\\i {}/SCRIPTS", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "Scripts/"));

		// Test mixed case matching
		let input = format!("\\i {}/ReAdMe", path_str);
		let completions = completer.find_completions(&input, input.len());

		assert!(!completions.is_empty());
		assert!(completions.iter().any(|c| c.display == "README.md"));

		// Cleanup
		let _ = fs::remove_dir_all(&temp_dir);
	}

	#[test]
	fn test_output_command_completion() {
		let completer = SqlCompleter::new(Theme::Dark);
		let completions = completer.find_completions("\\", 1);
		assert!(completions.iter().any(|c| c.display == "\\o"));
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
		let input = format!("\\o {}/output", path_str);
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
