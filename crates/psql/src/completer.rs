//! SQL and psql command completion for rustyline
//!
//! This module provides autocompletion functionality for the psql wrapper,
//! implementing rustyline's `Completer` trait to offer suggestions as users type.
//!
//! # Features
//!
//! - **SQL Keyword Completion**: Autocompletes common SQL keywords (SELECT, FROM, WHERE, etc.)
//! - **Data Type Completion**: Suggests PostgreSQL data types (INTEGER, TEXT, JSONB, etc.)
//! - **Function Completion**: Completes common SQL functions (COUNT, COALESCE, NOW, etc.)
//! - **psql Command Completion**: Autocompletes backslash commands (\\dt, \\d, \\l, etc.)
//! - **Case Insensitive**: Works regardless of input case
//!
//! # Usage
//!
//! Press `Tab` while typing to trigger autocompletion. The completer will:
//! - Show all matching SQL keywords when typing SQL commands
//! - Show matching psql commands when input starts with backslash
//! - Provide case-insensitive matching for SQL keywords
//!
//! # Limitations
//!
//! This is a static completer that doesn't query the database for schema information.
//! Unlike native psql's completion, it cannot suggest:
//! - Actual table names from your database
//! - Column names from specific tables
//! - Schema names
//! - Function names defined in your database
//!
//! The completer provides a baseline SQL keyword and command completion experience
//! without requiring database connectivity or query overhead.
//!
//! # Examples
//!
//! ```text
//! # Typing "SEL" + Tab suggests:
//! SELECT
//!
//! # Typing "\\d" + Tab suggests:
//! \d    \d+   \da   \db   \dc   \dt   \di   ...
//!
//! # Typing "select * fro" + Tab suggests:
//! FROM
//! ```

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};
use std::borrow::Cow;

/// SQL keywords and psql commands for autocompletion
pub struct SqlCompleter {
	keywords: Vec<&'static str>,
	psql_commands: Vec<&'static str>,
}

impl SqlCompleter {
	pub fn new() -> Self {
		Self {
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
		}

		// Sort completions alphabetically
		completions.sort_by(|a, b| a.display.cmp(&b.display));
		completions
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
		// Could implement hints showing the first completion candidate
		None
	}
}

impl Highlighter for SqlCompleter {
	fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
		// No syntax highlighting for now
		Cow::Borrowed(line)
	}

	fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
		false
	}
}

impl Validator for SqlCompleter {
	fn validate(
		&self,
		_ctx: &mut rustyline::validate::ValidationContext,
	) -> rustyline::Result<rustyline::validate::ValidationResult> {
		Ok(rustyline::validate::ValidationResult::Valid(None))
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
