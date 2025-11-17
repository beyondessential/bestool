use miette::{Diagnostic, LabeledSpan, NamedSource, SourceCode};
use tokio_postgres::error::{DbError, Error};

use crate::pool::PgError;

/// Custom error type for formatting PostgreSQL database errors as miette diagnostics
#[derive(Debug, thiserror::Error)]
pub struct PgDatabaseError {
	message: String,
	hint: Option<String>,
	source_code: Option<NamedSource<String>>,
	label: Option<miette::SourceSpan>,
	label_text: String,
	severity: String,
	code: String,
	detail: Option<String>,
	where_clause: Option<String>,
	schema: Option<String>,
	table: Option<String>,
	column: Option<String>,
	datatype: Option<String>,
	constraint: Option<String>,
	file: Option<String>,
	line: Option<u32>,
	routine: Option<String>,
}

impl std::fmt::Display for PgDatabaseError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		writeln!(f, "{}: {}", self.severity, self.message)?;
		writeln!(f, "  Code: {}", self.code)?;

		if let Some(detail) = &self.detail {
			writeln!(f, "  Detail: {}", detail)?;
		}
		if let Some(where_clause) = &self.where_clause {
			writeln!(f, "  Where: {}", where_clause)?;
		}
		if let Some(schema) = &self.schema {
			writeln!(f, "  Schema: {}", schema)?;
		}
		if let Some(table) = &self.table {
			writeln!(f, "  Table: {}", table)?;
		}
		if let Some(column) = &self.column {
			writeln!(f, "  Column: {}", column)?;
		}
		if let Some(datatype) = &self.datatype {
			writeln!(f, "  Datatype: {}", datatype)?;
		}
		if let Some(constraint) = &self.constraint {
			writeln!(f, "  Constraint: {}", constraint)?;
		}
		if let Some(hint) = &self.hint {
			writeln!(f, "  Hint: {}", hint)?;
		}
		if let Some(file) = &self.file {
			write!(f, "  Source: {}", file)?;
			if let Some(line) = self.line {
				write!(f, ":{}", line)?;
			}
			if let Some(routine) = &self.routine {
				write!(f, " in {}", routine)?;
			}
			writeln!(f)?;
		}

		Ok(())
	}
}

impl Diagnostic for PgDatabaseError {
	fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
		self.hint
			.as_ref()
			.map(|h| Box::new(h.clone()) as Box<dyn std::fmt::Display>)
	}

	fn source_code(&self) -> Option<&dyn SourceCode> {
		self.source_code.as_ref().map(|s| s as &dyn SourceCode)
	}

	fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
		if let Some(span) = self.label {
			Some(Box::new(std::iter::once(LabeledSpan::new_with_span(
				Some(self.label_text.clone()),
				span,
			))))
		} else {
			None
		}
	}
}

impl PgDatabaseError {
	/// Create a PgDatabaseError from a tokio-postgres DbError
	pub fn from_db_error(db_error: &DbError, query: Option<&str>) -> Self {
		let message = db_error.message().to_string();
		let hint = db_error.hint().map(|s| s.to_string());
		let detail = db_error.detail().map(|s| s.to_string());
		let code = db_error.code().code().to_string();
		let severity = db_error
			.parsed_severity()
			.map(|s| format!("{:?}", s))
			.unwrap_or_else(|| db_error.severity().to_string());
		let position = db_error.position().map(|pos| match pos {
			tokio_postgres::error::ErrorPosition::Original(p) => format!("position {}", p),
			tokio_postgres::error::ErrorPosition::Internal { position, query } => {
				format!("internal position {} in query: {}", position, query)
			}
		});
		let where_clause = db_error.where_().map(|s| s.to_string());
		let schema = db_error.schema().map(|s| s.to_string());
		let table = db_error.table().map(|s| s.to_string());
		let column = db_error.column().map(|s| s.to_string());
		let datatype = db_error.datatype().map(|s| s.to_string());
		let constraint = db_error.constraint().map(|s| s.to_string());
		let file = db_error.file().map(|s| s.to_string());
		let line = db_error.line();
		let routine = db_error.routine().map(|s| s.to_string());

		// Create source code and label if we have both query and position
		if let (Some(query_str), Some(pos_str)) = (query, &position)
			&& let Some(pos_str) = pos_str.strip_prefix("position ")
			&& let Ok(pos) = pos_str.parse::<usize>()
		{
			// PostgreSQL positions are 1-based, convert to 0-based
			let pos_zero_based = pos.saturating_sub(1);

			// Ensure position is within query bounds
			let actual_pos = pos_zero_based.min(query_str.len().saturating_sub(1));

			let source = NamedSource::new("query", query_str.to_string());
			let span = miette::SourceSpan::from(actual_pos..actual_pos + 1);
			return Self {
				message,
				hint,
				source_code: Some(source),
				label: Some(span),
				label_text: "error here".to_string(),
				severity,
				code,
				detail,
				where_clause,
				schema,
				table,
				column,
				datatype,
				constraint,
				file,
				line,
				routine,
			};
		}

		Self {
			message,
			hint,
			source_code: None,
			label: None,
			label_text: String::new(),
			severity,
			code,
			detail,
			where_clause,
			schema,
			table,
			column,
			datatype,
			constraint,
			file,
			line,
			routine,
		}
	}
}

/// Format a mobc<tokio-postgres> Error for display
pub fn format_mobc_error(error: &mobc::Error<PgError>, query: Option<&str>) -> String {
	match error {
		mobc::Error::Inner(PgError::Pg(error)) => format_db_error(error, query),
		error => format_error(error),
	}
}

/// Format a tokio-postgres Error for display
pub fn format_db_error(error: &Error, query: Option<&str>) -> String {
	match error.as_db_error() {
		Some(db_error) => {
			let pg_error = PgDatabaseError::from_db_error(db_error, query);

			// If we have source code, use miette's fancy formatting
			if pg_error.source_code.is_some() {
				format!("{:?}", miette::Report::new(pg_error))
			} else {
				// Otherwise use our custom Display implementation
				format!("{}", pg_error)
			}
		}
		None => format!("{:?}", error),
	}
}

/// Format any error for display
pub fn format_error(error: &dyn std::fmt::Debug) -> String {
	format!("{:?}", error)
}

/// Format a miette Report for display, extracting database errors if present
pub fn format_miette_error(report: &miette::Report, query: Option<&str>) -> String {
	// Try to downcast to PgDatabaseError first (already has query and all details)
	if let Some(pg_db_error) = report.downcast_ref::<PgDatabaseError>() {
		// If we have source code, use miette's fancy formatting
		if pg_db_error.source_code.is_some() {
			return format!("{:?}", report);
		} else {
			// Otherwise use our custom Display implementation
			return format!("{}", pg_db_error);
		}
	}

	// Try to downcast to tokio_postgres::Error
	if let Some(pg_error) = report.downcast_ref::<Error>() {
		return format_db_error(pg_error, query);
	}

	// Otherwise, format the report
	// Use Debug formatting which will use miette's fancy output if available
	format!("{:?}", report)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_format_db_error_with_real_error() {
		// This test requires DATABASE_URL to be set
		let database_url = std::env::var("DATABASE_URL")
			.unwrap_or_else(|_| "postgresql://localhost/postgres".to_string());

		let (client, connection) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls)
			.await
			.expect("Failed to connect to database");

		tokio::spawn(async move {
			if let Err(e) = connection.await {
				eprintln!("Connection error: {}", e);
			}
		});

		// Execute a query that will definitely fail with a syntax error
		let result = client.query("SELECT 1+;", &[]).await;

		assert!(result.is_err(), "Query should have failed");

		let error = result.unwrap_err();
		let formatted = format_db_error(&error, Some("SELECT 1+;"));

		// Check that the formatted error contains expected information
		assert!(formatted.contains("ERROR") || formatted.contains("syntax error"));
		assert!(formatted.contains("Code:"));

		println!("Formatted error:\n{}", formatted);
	}

	#[test]
	fn test_pg_database_error_display() {
		let error = PgDatabaseError {
			message: "syntax error at end of input".to_string(),
			hint: Some("Check your SQL syntax".to_string()),
			source_code: None,
			label: None,
			label_text: String::new(),
			severity: "ERROR".to_string(),
			code: "42601".to_string(),
			detail: Some("The query ended unexpectedly".to_string()),
			where_clause: None,
			schema: None,
			table: Some("users".to_string()),
			column: None,
			datatype: None,
			constraint: None,
			file: Some("parser.c".to_string()),
			line: Some(123),
			routine: Some("parse_query".to_string()),
		};

		let output = format!("{}", error);
		assert!(output.contains("ERROR: syntax error at end of input"));
		assert!(output.contains("Code: 42601"));
		assert!(output.contains("Detail: The query ended unexpectedly"));
		assert!(output.contains("Table: users"));
		assert!(output.contains("Hint: Check your SQL syntax"));
		assert!(output.contains("Source: parser.c:123 in parse_query"));
	}

	#[test]
	fn test_pg_database_error_minimal() {
		let error = PgDatabaseError {
			message: "column does not exist".to_string(),
			hint: None,
			source_code: None,
			label: None,
			label_text: String::new(),
			severity: "ERROR".to_string(),
			code: "42703".to_string(),
			detail: None,
			where_clause: None,
			schema: None,
			table: None,
			column: None,
			datatype: None,
			constraint: None,
			file: None,
			line: None,
			routine: None,
		};

		let output = format!("{}", error);
		assert!(output.contains("ERROR: column does not exist"));
		assert!(output.contains("Code: 42703"));
		// Should not contain optional fields
		assert!(!output.contains("Detail:"));
		assert!(!output.contains("Hint:"));
		assert!(!output.contains("Source:"));
	}

	#[tokio::test]
	async fn test_complete_error_output() {
		// This test demonstrates the complete error output with all fields
		let database_url = std::env::var("DATABASE_URL")
			.unwrap_or_else(|_| "postgresql://localhost/postgres".to_string());

		let (client, connection) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls)
			.await
			.expect("Failed to connect to database");

		tokio::spawn(async move {
			if let Err(e) = connection.await {
				eprintln!("Connection error: {}", e);
			}
		});

		// Test 1: Syntax error
		let result = client.query("SELECT 1+;", &[]).await;
		if let Err(e) = result {
			if let Some(db_error) = e.as_db_error() {
				let pg_error = PgDatabaseError::from_db_error(db_error, Some("SELECT 1+;"));
				let output = format!("{}", pg_error);
				println!("=== Syntax Error Output ===\n{}", output);
				assert!(output.contains("Code:"));
				assert!(output.contains("Position:") || output.contains("Source:"));
			}
		}

		// Test 2: Column does not exist
		let result = client.query("SELECT nonexistent FROM pg_class", &[]).await;
		if let Err(e) = result {
			if let Some(db_error) = e.as_db_error() {
				let pg_error = PgDatabaseError::from_db_error(
					db_error,
					Some("SELECT nonexistent FROM pg_class"),
				);
				let output = format!("{}", pg_error);
				println!("=== Column Error Output ===\n{}", output);
				assert!(output.contains("Code:"));
			}
		}
	}

	#[test]
	fn test_miette_span_rendering() {
		// Test to verify miette renders the span at the correct position
		let query = "SELECT 1+;";

		// Test 1: Hardcoded span at bytes 3-5 (should highlight "EC")
		let error = PgDatabaseError {
			message: "syntax error".to_string(),
			hint: None,
			source_code: Some(NamedSource::new("query", query.to_string())),
			label: Some(miette::SourceSpan::from(3..5)),
			label_text: "at bytes 3-5".to_string(),
			severity: "ERROR".to_string(),
			code: "42601".to_string(),
			detail: None,
			where_clause: None,
			schema: None,
			table: None,
			column: None,
			datatype: None,
			constraint: None,
			file: None,
			line: None,
			routine: None,
		};

		let report = miette::Report::new(error);
		let formatted = format!("{:?}", report);
		println!("Test 1 - Span at bytes 3-5:\n{}", formatted);

		// Test 2: Span at the semicolon (byte 9)
		let error2 = PgDatabaseError {
			message: "syntax error".to_string(),
			hint: None,
			source_code: Some(NamedSource::new("query", query.to_string())),
			label: Some(miette::SourceSpan::from(9..10)),
			label_text: "semicolon".to_string(),
			severity: "ERROR".to_string(),
			code: "42601".to_string(),
			detail: None,
			where_clause: None,
			schema: None,
			table: None,
			column: None,
			datatype: None,
			constraint: None,
			file: None,
			line: None,
			routine: None,
		};

		let report2 = miette::Report::new(error2);
		let formatted2 = format!("{:?}", report2);
		println!("Test 2 - Span at byte 9 (semicolon):\n{}", formatted2);

		// Verify the query is in the output
		assert!(formatted.contains("SELECT"));
		assert!(formatted2.contains("SELECT"));
	}
}
