mod audit;
mod completer;
mod config;
mod highlighter;
mod input;
mod ots;
mod parser;
mod pool;
mod query;
mod repl;
mod schema_cache;
mod tls;

pub use config::{PsqlConfig, PsqlError};
pub use highlighter::Theme;
pub use pool::{create_pool, PgConnection, PgPool};
pub use repl::run;

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
pub(crate) fn sigint_received() -> bool {
	SIGINT_RECEIVED.load(Ordering::SeqCst)
}

#[cfg(unix)]
pub(crate) fn reset_sigint() {
	SIGINT_RECEIVED.store(false, Ordering::SeqCst);
}

#[cfg(unix)]
pub fn register_sigint_handler() -> Result<(), std::io::Error> {
	use signal_hook::consts::SIGINT;

	unsafe {
		signal_hook::low_level::register(SIGINT, || {
			SIGINT_RECEIVED.store(true, Ordering::SeqCst);
		})?;
	}
	Ok(())
}
