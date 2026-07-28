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

/// A check's declared heal: what to run, and the minimum interval between its
/// attempts. The interval is a floor applied after every attempt — including
/// one straight after a successful repair — so a repair whose effect reaches
/// the check only slowly cannot loop into repeated repairs.
#[derive(Clone, Copy)]
pub struct HealAction {
	pub run: HealFn,
	pub min_interval: Duration,
}

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

/// Default minimum wait between a check's heal attempts, so a check that cannot
/// yet be healed doesn't retry on every sweep. A check whose repair is
/// disruptive or slow-acting declares a longer floor of its own.
pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Longest the backoff grows to, unless a check's own minimum interval is
/// longer (which then wins).
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

/// The delay before the next attempt after `failures` consecutive non-heals,
/// for a check whose minimum interval is `min_interval`: the interval doubling,
/// never below the interval and never above [`MAX_INTERVAL`] (or the interval
/// itself, when that is the longer of the two).
fn backoff_delay(failures: u32, min_interval: Duration) -> Duration {
	let steps = failures.saturating_sub(1).min(u32::BITS - 1);
	min_interval
		.saturating_mul(2u32.saturating_pow(steps))
		.clamp(min_interval, MAX_INTERVAL.max(min_interval))
}

/// Spawn `action` for `name` in the background if it is due and not already
/// running. A no-op when a previous attempt is still in flight or the backoff
/// window has not elapsed, so the caller can invoke it on every sweep.
pub fn spawn_if_due(name: &'static str, action: HealAction, ctx: SweepContext) {
	if !try_begin(name) {
		return;
	}
	debug!(check = name, "spawning self-heal attempt");
	tokio::spawn(async move {
		let outcome = (action.run)(ctx).await;
		finish(name, outcome, action.min_interval);
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
/// the failure run, anything else advances it. Either way the next attempt is
/// held off for at least `min_interval` — a successful repair is not retried
/// immediately, so a repair whose effect is not yet visible cannot loop.
fn finish(name: &'static str, outcome: HealOutcome, min_interval: Duration) {
	let mut map = registry().lock().expect("heal registry poisoned");
	let attempt = map.entry(name).or_default();
	attempt.in_flight = false;
	attempt.failures = match outcome {
		HealOutcome::Healed => 0,
		HealOutcome::Deferred | HealOutcome::Failed => attempt.failures.saturating_add(1),
	};
	attempt.next_allowed = Some(Instant::now() + backoff_delay(attempt.failures, min_interval));
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn backoff_grows_and_caps() {
		let d = DEFAULT_MIN_INTERVAL;
		assert_eq!(backoff_delay(0, d), d);
		assert_eq!(backoff_delay(1, d), d);
		assert_eq!(backoff_delay(2, d), d * 2);
		assert_eq!(backoff_delay(3, d), d * 4);
		// Caps at MAX_INTERVAL rather than growing without bound.
		assert_eq!(backoff_delay(100, d), MAX_INTERVAL);
	}

	#[test]
	fn a_longer_min_interval_is_the_floor_and_cap() {
		// A check with an hourly floor never attempts more often than hourly,
		// even on its first backoff step, and its floor outranks MAX_INTERVAL.
		let hour = Duration::from_secs(60 * 60);
		assert_eq!(backoff_delay(0, hour), hour);
		assert_eq!(backoff_delay(1, hour), hour);
		assert_eq!(backoff_delay(100, hour), hour);
	}

	#[test]
	fn at_most_one_attempt_in_flight() {
		// Distinct name so the process-global registry can't collide with
		// another test; a zero interval isolates the in-flight guard from the
		// backoff floor.
		let name = "test_in_flight";
		let zero = Duration::ZERO;
		assert!(try_begin(name), "first attempt is due");
		assert!(
			!try_begin(name),
			"a second attempt is refused while in flight"
		);
		finish(name, HealOutcome::Healed, zero);
		assert!(try_begin(name), "with no floor the check is due again");
		finish(name, HealOutcome::Healed, zero);
	}

	#[test]
	fn deferred_attempt_backs_off() {
		let name = "test_backoff";
		assert!(try_begin(name), "first attempt is due");
		finish(name, HealOutcome::Deferred, DEFAULT_MIN_INTERVAL);
		// The next attempt is scheduled into the future, so it is not due now.
		assert!(
			!try_begin(name),
			"a deferred attempt backs off rather than retrying immediately"
		);
	}

	#[test]
	fn a_successful_repair_still_waits_the_min_interval() {
		// The floor applies even after a heal, so a repair whose effect is not
		// yet visible to the check cannot trigger a second repair right away.
		let name = "test_heal_floor";
		assert!(try_begin(name), "first attempt is due");
		finish(name, HealOutcome::Healed, Duration::from_secs(60 * 60));
		assert!(
			!try_begin(name),
			"a healed check waits its minimum interval before the next attempt"
		);
	}
}
