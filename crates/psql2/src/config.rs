use crate::highlighter::Theme;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PsqlError {
	#[error("database connection failed")]
	ConnectionFailed,
	#[error("query execution failed")]
	QueryFailed,
}

/// Configuration for the psql2 client
#[derive(Debug, Clone)]
pub struct PsqlConfig {
	/// Database connection string
	pub connection_string: String,

	/// Database user for tracking
	pub user: Option<String>,

	/// Syntax highlighting theme
	pub theme: Theme,

	/// Path to history database
	pub history_path: std::path::PathBuf,

	/// Database name for display in prompt
	pub database_name: String,

	/// Whether write mode is enabled
	pub write: bool,

	/// OTS (Over The Shoulder) value for write mode sessions
	pub ots: Option<String>,
}
