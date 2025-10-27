use miette::Result;
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
}

/// Run the psql2 client
pub async fn run(_config: PsqlConfig) -> Result<()> {
	Ok(())
}
