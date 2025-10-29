use thiserror::Error;

use crate::{highlighter::Theme, pool::PgPool};

#[derive(Debug, Error)]
pub enum PsqlError {
	#[error("database connection failed")]
	ConnectionFailed,
	#[error("query execution failed")]
	QueryFailed,
}

/// Configuration for the psql2 client
#[derive(Clone, Debug)]
pub struct PsqlConfig {
	/// Database connection pool
	pub pool: PgPool,

	/// Database user for tracking
	pub user: Option<String>,

	/// Syntax highlighting theme
	pub theme: Theme,

	/// Path to audit database
	pub audit_path: Option<std::path::PathBuf>,

	/// Database name for display in prompt
	pub database_name: String,

	/// Whether write mode is enabled upon entering the REPL
	pub write: bool,

	/// Whether to use colours in output
	pub use_colours: bool,
}
