use std::sync::atomic::{AtomicBool, Ordering};

static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

pub(crate) fn sigint_received() -> bool {
	SIGINT_RECEIVED.load(Ordering::SeqCst)
}

pub(crate) fn reset_sigint() {
	SIGINT_RECEIVED.store(false, Ordering::SeqCst);
}

pub fn register_sigint_handler() -> miette::Result<()> {
	ctrlc::set_handler(move || {
		SIGINT_RECEIVED.store(true, Ordering::SeqCst);
	})
	.map_err(|e| miette::miette!("Failed to register Ctrl-C handler: {e}"))
}
