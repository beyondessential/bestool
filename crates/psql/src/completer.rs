use std::{
	borrow::Cow,
	collections::VecDeque,
	io::Write,
	sync::{Arc, Mutex, RwLock},
};

use rustyline::highlight::CmdKind;
use rustyline::{
	completion::{Completer, Pair},
	highlight::Highlighter,
	hint::Hinter,
	validate::{ValidationContext, ValidationResult, Validator},
	Context, Helper,
};
use syntect::{
	easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet,
	util::as_24_bit_terminal_escaped,
};

use crate::{highlighter::Theme, schema_cache::SchemaCache};

/// SQL keywords and psql commands for autocompletion
pub struct SqlCompleter {
	keywords: Vec<&'static str>,
	psql_commands: Vec<&'static str>,
	pty_writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
	pty_buffer: Option<Arc<Mutex<VecDeque<u8>>>>,
	/// Cached schema information from the database
	schema_cache: Option<Arc<RwLock<SchemaCache>>>,
	/// Syntax highlighting support
	syntax_set: SyntaxSet,
	theme_set: ThemeSet,
	theme: Theme,
}

impl SqlCompleter {
	pub fn new() -> Self {
		Self::with_theme(Theme::Dark)
	}

	/// Create a new SqlCompleter with a specific theme
	pub fn with_theme(theme: Theme) -> Self {
		Self {
			pty_writer: None,
			pty_buffer: None,
			schema_cache: None,
			syntax_set: SyntaxSet::load_defaults_newlines(),
			theme_set: ThemeSet::load_defaults(),
			theme,
			keywords: vec![
				// SQL Keywords (uppercase for convention)
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
				// Common data types
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
				// Common functions
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
				// Postgres specific
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
				// Meta-commands
				"\\?",
				"\\h",
				"\\q",
				"\\c",
				"\\d",
				"\\dt",
				"\\di",
				"\\dv",
				"\\ds",
				"\\df",
				"\\dT",
				"\\du",
				"\\dn",
				"\\dp",
				"\\l",
				"\\z",
				"\\d+",
				"\\dt+",
				"\\di+",
				"\\dv+",
				"\\ds+",
				"\\df+",
				"\\dT+",
				"\\du+",
				"\\dn+",
				"\\dp+",
				"\\l+",
				"\\da",
				"\\db",
				"\\dc",
				"\\dC",
				"\\dd",
				"\\dD",
				"\\ddp",
				"\\dE",
				"\\des",
				"\\det",
				"\\deu",
				"\\dew",
				"\\dF",
				"\\dFd",
				"\\dFp",
				"\\dFt",
				"\\dg",
				"\\dL",
				"\\dm",
				"\\do",
				"\\dO",
				"\\drds",
				"\\dRs",
				"\\dRp",
				"\\dy",
				"\\e",
				"\\ef",
				"\\ev",
				"\\edit",
				"\\echo",
				"\\qecho",
				"\\warn",
				"\\i",
				"\\ir",
				"\\include",
				"\\include_relative",
				"\\o",
				"\\out",
				"\\p",
				"\\print",
				"\\r",
				"\\reset",
				"\\s",
				"\\history",
				"\\w",
				"\\write",
				"\\x",
				"\\expanded",
				"\\g",
				"\\go",
				"\\gx",
				"\\gexec",
				"\\gset",
				"\\watch",
				"\\timing",
				"\\t",
				"\\tuples_only",
				"\\a",
				"\\aligned",
				"\\C",
				"\\caption",
				"\\f",
				"\\fieldsep",
				"\\fieldsep_zero",
				"\\H",
				"\\html",
				"\\T",
				"\\tableattr",
				"\\pset",
				"\\P",
				"\\pager",
				"\\encoding",
				"\\password",
				"\\cd",
				"\\setenv",
				"\\!",
				"\\shell",
				"\\copy",
				"\\crosstabview",
				"\\errverbose",
				"\\gdesc",
				"\\set",
				"\\unset",
				"\\prompt",
				"\\if",
				"\\elif",
				"\\else",
				"\\endif",
				"\\lo_import",
				"\\lo_export",
				"\\lo_list",
				"\\lo_unlink",
				"\\conninfo",
				"\\connect",
				// Custom bestool commands
				"\\W",
			],
		}
	}

	/// Find completions for the given input
	fn find_completions(&self, input: &str, pos: usize) -> Vec<Pair> {
		let text_before_cursor = &input[..pos];

		// Find the start of the current word
		let word_start = text_before_cursor
			.rfind(|c: char| c.is_whitespace() || c == '(' || c == ',' || c == ';')
			.map(|i| i + 1)
			.unwrap_or(0);

		let current_word = &text_before_cursor[word_start..];

		if current_word.is_empty() {
			return Vec::new();
		}

		let mut completions = Vec::new();

		// Check if we're completing a psql command (starts with backslash)
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

			// Check if we're in a FROM clause (suggest tables)
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

			// Add schema names if we have them
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

			// Complete SQL keywords
			let current_upper = current_word.to_uppercase();
			for keyword in &self.keywords {
				if keyword.starts_with(&current_upper) {
					completions.push(Pair {
						display: keyword.to_string(),
						replacement: keyword.to_string(),
					});
				}
			}

			// Add table names from schema cache
			if let Some(cache) = &self.schema_cache {
				let cache = cache.read().unwrap();
				for table in cache.all_tables() {
					if table
						.to_lowercase()
						.starts_with(&current_word.to_lowercase())
					{
						// Avoid duplicates if already added in FROM clause detection
						if !completions.iter().any(|c| c.display == table) {
							completions.push(Pair {
								display: table.clone(),
								replacement: table,
							});
						}
					}
				}
			}

			// Add function names from schema cache
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

		// Sort completions alphabetically, with underscores sorting after alphanumerics
		completions.sort_by(|a, b| {
			// Create versions where underscores are replaced with a character that sorts after 'z'
			let a_key = a.display.replace('_', "~");
			let b_key = b.display.replace('_', "~");
			a_key.cmp(&b_key)
		});
		completions.dedup_by(|a, b| a.display == b.display);
		completions
	}

	/// Create a completer with PTY and a specific theme
	pub fn with_pty_and_theme(
		pty_writer: Arc<Mutex<Box<dyn Write + Send>>>,
		pty_buffer: Arc<Mutex<VecDeque<u8>>>,
		theme: Theme,
	) -> Self {
		let mut completer = Self::with_theme(theme);
		completer.pty_writer = Some(pty_writer);
		completer.pty_buffer = Some(pty_buffer);
		completer
	}

	/// Set the schema cache for database-aware completion
	pub fn with_schema_cache(mut self, cache: Arc<RwLock<SchemaCache>>) -> Self {
		self.schema_cache = Some(cache);
		self
	}
}

impl Default for SqlCompleter {
	fn default() -> Self {
		Self::new()
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

		// Find the start of the current word for replacement position
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
		// Get SQL syntax
		let syntax = self
			.syntax_set
			.find_syntax_by_extension("sql")
			.unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

		// Get theme name (resolve Auto to Light or Dark)
		let resolved_theme = self.theme.resolve();
		let theme_name = match resolved_theme {
			Theme::Light => "base16-ocean.light",
			Theme::Dark => "base16-ocean.dark",
			Theme::Auto => unreachable!("resolve() always returns Light or Dark"),
		};

		// Get theme
		let theme = &self.theme_set.themes[theme_name];

		// Highlight
		let mut highlighter = HighlightLines::new(syntax, theme);
		match highlighter.highlight_line(line, &self.syntax_set) {
			Ok(ranges) => {
				let mut escaped = as_24_bit_terminal_escaped(&ranges[..], false);
				// Add ANSI reset sequence to prevent color bleeding into prompt/output
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
		let completer = SqlCompleter::new();
		let completions = completer.find_completions("SEL", 3);
		assert!(completions.iter().any(|c| c.display == "SELECT"));
	}

	#[test]
	fn test_psql_command_completion() {
		let completer = SqlCompleter::new();
		let completions = completer.find_completions("\\d", 2);
		assert!(completions.len() > 0);
		assert!(completions.iter().any(|c| c.display == "\\dt"));
	}

	#[test]
	fn test_case_insensitive_completion() {
		let completer = SqlCompleter::new();
		let completions = completer.find_completions("select", 6);
		assert!(completions.iter().any(|c| c.display == "SELECT"));
	}

	#[test]
	fn test_mid_query_completion() {
		let completer = SqlCompleter::new();
		let completions = completer.find_completions("SELECT * FRO", 12);
		assert!(completions.iter().any(|c| c.display == "FROM"));
	}

	#[test]
	fn test_custom_command_completion() {
		let completer = SqlCompleter::new();
		let completions = completer.find_completions("\\W", 2);
		assert!(completions.iter().any(|c| c.display == "\\W"));
	}
}
