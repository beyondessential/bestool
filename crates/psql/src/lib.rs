#![deny(rust_2018_idioms)]

mod audit;
mod completer;
mod config;
mod input;
mod ots;
mod parser;
mod pool;
mod query;
mod repl;
mod schema_cache;
mod signals;
mod snippets;
mod table;
mod theme;
mod tls;

pub use config::Config;
pub use pool::{PgConnection, PgPool, create_pool};
pub use repl::run;
pub use signals::register_sigint_handler;
pub use theme::Theme;
