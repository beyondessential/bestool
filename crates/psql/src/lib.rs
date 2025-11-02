#![deny(rust_2018_idioms)]

pub mod audit;
mod completer;
mod config;
mod error;
mod input;
mod ots;
mod parser;
mod pool;
mod query;
pub mod repl;
pub mod result_store;
mod schema_cache;
mod signals;
pub mod snippets;
mod table;
mod theme;
mod tls;

pub use audit::{ExportOptions, QueryOptions, export_audit_entries};
pub use config::Config;
pub use pool::{PgConnection, PgPool, create_pool};
pub use repl::run;
pub use signals::register_sigint_handler;
pub use theme::Theme;

pub fn default_audit_dir() -> String {
	audit::Audit::help_text_default_dir()
}
