//! FHIR integration / worker consistency.
//!
//! Tamanu's FHIR support has two independent toggles: `integrations.fhir.enabled`
//! gates whether materialisation jobs are queued, and
//! `integrations.fhir.worker.enabled` runs the worker that consumes that queue.
//! They're meant to move together.
//!
//! With the API enabled but the worker off, jobs are queued with nothing to
//! consume them and the queue grows unbounded — a real fault, so we fail. The
//! inverse (worker on, API off) means the worker has no jobs to do but still
//! holds DB connections and server memory; wasteful but not harmful, so it only
//! warns. Both on or both off is consistent and passes.

use super::CheckContext;
use crate::doctor::check::Check;

const NAME: &str = "fhir_config";

pub async fn run(ctx: CheckContext) -> Check {
	if !ctx.has_install {
		return Check::skip(
			NAME,
			"no Tamanu config on this host",
			"the FHIR toggles live in the install's config, and this context was built from a database URL alone",
		);
	}
	evaluate(ctx.config.fhir_enabled(), ctx.config.fhir_worker_enabled())
}

fn evaluate(fhir_enabled: bool, worker_enabled: bool) -> Check {
	let check = match (fhir_enabled, worker_enabled) {
		(true, false) => Check::fail(
			NAME,
			"FHIR enabled but its worker is off",
			"integrations.fhir.enabled is true while integrations.fhir.worker.enabled is false: materialisation jobs are queued with no worker to consume them, so the queue grows unbounded",
		),
		(false, true) => Check::warning(
			NAME,
			"FHIR worker on but FHIR is off",
			"integrations.fhir.worker.enabled is true while integrations.fhir.enabled is false: the worker has no jobs to do but still holds DB connections and server memory",
		),
		(true, true) => Check::pass(NAME, "FHIR and its worker both enabled"),
		(false, false) => Check::pass(NAME, "FHIR and its worker both disabled"),
	};
	check
		.with_detail("fhir_enabled", fhir_enabled)
		.with_detail("worker_enabled", worker_enabled)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::check::CheckStatus;

	#[test]
	fn fhir_on_worker_off_fails() {
		assert!(matches!(evaluate(true, false).status, CheckStatus::Fail(_)));
	}

	#[test]
	fn fhir_off_worker_on_warns() {
		assert!(matches!(
			evaluate(false, true).status,
			CheckStatus::Warning(_)
		));
	}

	#[test]
	fn both_on_passes() {
		assert!(matches!(evaluate(true, true).status, CheckStatus::Pass));
	}

	#[test]
	fn both_off_passes() {
		assert!(matches!(evaluate(false, false).status, CheckStatus::Pass));
	}
}
