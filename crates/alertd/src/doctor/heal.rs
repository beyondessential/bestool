//! Self-heal actions for healthchecks.
//!
//! A check may declare an optional heal action the daemon runs, in the
//! background, when the check is not passing — a repair that recovers the
//! condition the check grades without operator action. The action never
//! changes the outcome reported for the sweep that triggered it; a successful
//! repair shows up on a later sweep once the healed condition is observed
//! afresh.
//!
//! Attempts are rate-limited per check and back off on repeated failure, and at
//! most one attempt for a given check runs at a time — because attempts run in
//! the background, one can outlast the interval between sweeps.
//!
//! spec: CHK#self-healing

use std::{
	collections::HashMap,
	sync::{Mutex, OnceLock},
	time::{Duration, Instant},
};

use futures::future::BoxFuture;
use tracing::debug;

use super::checks::SweepContext;

/// A check's heal action: mirrors the check's own runner, returning what the
/// attempt achieved rather than a graded outcome.
pub type HealFn = fn(SweepContext) -> BoxFuture<'static, HealOutcome>;

/// The result of one heal attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealOutcome {
	/// The repair was made; the check will observe the healed condition on a
	/// later sweep. Resets the backoff.
	Healed,
	/// The repair could not proceed this time — a dependency was unreachable,
	/// say, or there was nothing to act on. Backs off.
	Deferred,
	/// The attempt errored. Backs off.
	Failed,
}

/// Shortest wait between heal attempts for one check, so a check that cannot
/// yet be healed doesn't retry on every sweep.
const MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Longest the backoff grows to.
const MAX_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Per-check attempt state: whether an attempt is in flight, when the next is
/// allowed (`None` = allowed now), and the run of consecutive non-heals driving
/// the backoff.
#[derive(Default)]
struct Attempt {
	in_flight: bool,
	next_allowed: Option<Instant>,
	failures: u32,
}

fn registry() -> &'static Mutex<HashMap<&'static str, Attempt>> {
	static STATE: OnceLock<Mutex<HashMap<&'static str, Attempt>>> = OnceLock::new();
	STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The backoff delay after `failures` consecutive non-heals: [`MIN_INTERVAL`]
/// doubling up to [`MAX_INTERVAL`].
fn backoff_delay(failures: u32) -> Duration {
	let steps = failures.saturating_sub(1).min(u32::BITS - 1);
	MIN_INTERVAL
		.saturating_mul(2u32.saturating_pow(steps))
		.min(MAX_INTERVAL)
}

/// Spawn `heal` for `name` in the background if it is due and not already
/// running. A no-op when a previous attempt is still in flight or the backoff
/// window has not elapsed, so the caller can invoke it on every sweep.
pub fn spawn_if_due(name: &'static str, heal: HealFn, ctx: SweepContext) {
	if !try_begin(name) {
		return;
	}
	debug!(check = name, "spawning self-heal attempt");
	tokio::spawn(async move {
		let outcome = heal(ctx).await;
		finish(name, outcome);
	});
}

/// Reserve an attempt slot for `name`, returning whether the caller may run it.
/// Returns false when an attempt is in flight or the backoff has not elapsed.
fn try_begin(name: &'static str) -> bool {
	let mut map = registry().lock().expect("heal registry poisoned");
	let attempt = map.entry(name).or_default();
	if attempt.in_flight {
		return false;
	}
	if let Some(next) = attempt.next_allowed
		&& Instant::now() < next
	{
		return false;
	}
	attempt.in_flight = true;
	true
}

/// Release the attempt slot for `name` and record the outcome: a heal resets
/// the backoff, anything else advances it.
fn finish(name: &'static str, outcome: HealOutcome) {
	let mut map = registry().lock().expect("heal registry poisoned");
	let attempt = map.entry(name).or_default();
	attempt.in_flight = false;
	match outcome {
		HealOutcome::Healed => {
			attempt.failures = 0;
			attempt.next_allowed = None;
		}
		HealOutcome::Deferred | HealOutcome::Failed => {
			attempt.failures = attempt.failures.saturating_add(1);
			attempt.next_allowed = Some(Instant::now() + backoff_delay(attempt.failures));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn backoff_grows_and_caps() {
		assert_eq!(backoff_delay(0), MIN_INTERVAL);
		assert_eq!(backoff_delay(1), MIN_INTERVAL);
		assert_eq!(backoff_delay(2), MIN_INTERVAL * 2);
		assert_eq!(backoff_delay(3), MIN_INTERVAL * 4);
		// Caps at MAX_INTERVAL rather than growing without bound.
		assert_eq!(backoff_delay(100), MAX_INTERVAL);
	}

	#[test]
	fn at_most_one_attempt_in_flight() {
		// Distinct name so the process-global registry can't collide with
		// another test.
		let name = "test_in_flight";
		assert!(try_begin(name), "first attempt is due");
		assert!(
			!try_begin(name),
			"a second attempt is refused while in flight"
		);
		finish(name, HealOutcome::Healed);
		assert!(try_begin(name), "after a heal the check is due again");
		finish(name, HealOutcome::Healed);
	}

	#[test]
	fn deferred_attempt_backs_off() {
		let name = "test_backoff";
		assert!(try_begin(name), "first attempt is due");
		finish(name, HealOutcome::Deferred);
		// The next attempt is scheduled into the future, so it is not due now.
		assert!(
			!try_begin(name),
			"a deferred attempt backs off rather than retrying immediately"
		);
	}
}
