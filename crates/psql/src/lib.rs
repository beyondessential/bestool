#![deny(rust_2018_idioms)]
#![deny(unsafe_code)]

// #[doc(hidden)] items must not be used! Only pub for testing purposes.

#[doc(hidden)]
pub mod audit;
mod colors;
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

pub use audit::{ExportOptions, QueryOptions, export_audit_entries};
pub use bestool_postgres::pool::PgPool;
pub use config::Config;
pub use repl::run;
pub use signals::register_sigint_handler;
pub use theme::Theme;

/// Create a connection pool with application_name set to "bestool-psql"
pub async fn create_pool(url: &str) -> miette::Result<PgPool> {
	bestool_postgres::pool::create_pool(url, "bestool-psql").await
}

pub fn default_audit_dir() -> String {
	audit::Audit::help_text_default_dir()
}
