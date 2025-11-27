// #[doc(hidden)] items must not be used! Only pub for testing purposes.

#[doc(hidden)]
pub mod audit;
mod colors;
pub mod column_extractor;
mod completer;
mod config;
mod input;
mod ots;
mod parser;
mod query;
#[doc(hidden)]
pub mod repl;
#[doc(hidden)]
pub mod result_store;
mod schema_cache;
mod signals;
#[doc(hidden)]
pub mod snippets;
mod table;
mod theme;

use std::sync::Arc;

pub use audit::{ExportOptions, QueryOptions, export_audit_entries};
pub use bestool_postgres::pool::PgPool;
pub use config::Config;
pub use signals::register_sigint_handler;
pub use theme::Theme;

/// Create a connection pool with application_name set to "bestool-psql"
pub async fn create_pool(url: &str) -> miette::Result<PgPool> {
	bestool_postgres::pool::create_pool(url, "bestool-psql").await
}

pub fn default_audit_dir() -> String {
	audit::Audit::help_text_default_dir()
}

pub async fn run(pool: PgPool, mut config: Config) -> miette::Result<()> {
	if config.audit_path.is_none() {
		config.audit_path = Some(audit::Audit::default_path()?);
	}

	repl::run(pool, Arc::new(config)).await
}
