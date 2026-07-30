//! Daemon liveness tracking.
//!
//! Records the wall-clock second of the last activity so `/health` and the
//! watchdog can tell how stale the daemon is, and so `/metrics` can report the
//! age at scrape time. The `/metrics` renderer computes the age from this and
//! the scrape time; nothing exposes the raw timestamp as a metric.

use std::sync::atomic::{AtomicI64, Ordering};

use jiff::Timestamp;

static LAST_ACTIVITY: AtomicI64 = AtomicI64::new(0);

/// Record that the daemon did something just now.
pub fn record_activity() {
	LAST_ACTIVITY.store(Timestamp::now().as_second(), Ordering::Relaxed);
}

/// The unix second of the last recorded activity (`0` before the first).
pub fn last_activity_timestamp() -> i64 {
	LAST_ACTIVITY.load(Ordering::Relaxed)
}
