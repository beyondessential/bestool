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
mod snippets;
mod theme;
mod tls;

use std::sync::atomic::{AtomicBool, Ordering};

pub use config::{PsqlConfig, PsqlError};
pub use pool::{create_pool, PgConnection, PgPool};
pub use repl::run;
pub use theme::Theme;

static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

pub(crate) fn sigint_received() -> bool {
	SIGINT_RECEIVED.load(Ordering::SeqCst)
}

pub(crate) fn reset_sigint() {
	SIGINT_RECEIVED.store(false, Ordering::SeqCst);
}

pub fn register_sigint_handler() -> Result<(), Box<dyn std::error::Error>> {
	ctrlc::set_handler(move || {
		SIGINT_RECEIVED.store(true, Ordering::SeqCst);
	})?;
	Ok(())
}
