//! Reconcile canopy `billing.*` tags against the instance's IMDS tags.
//!
//! Billing attribution starts from the cloud provider's own instance tags, so
//! when canopy carries `billing.*` tags they must be reflected on the instance
//! itself. This check compares the two:
//!   * a `billing.*` tag in canopy that's absent from IMDS → warning (the
//!     instance isn't tagged for billing yet);
//!   * a `billing.*` tag whose IMDS value disagrees with canopy → failure
//!     (the instance is attributed to the wrong thing).
//!
//! The check only applies when both sides are present: it skips when canopy
//! carries no `billing.*` tags, and when IMDS tags aren't available (not on
//! EC2, or instance metadata tags disabled).

use std::collections::BTreeMap;

use serde_json::Value;

use bestool_tamanu::server_info::load_cached_tags;

use super::SweepContext;
use crate::doctor::{check::Check, server_info::fetch_imds_tags};

const CHECK_NAME: &str = "billing_tags";
const BILLING_PREFIX: &str = "billing.";

pub async fn run(_ctx: SweepContext) -> Check {
	let canopy_tags = load_cached_tags().unwrap_or_default();
	if !canopy_tags.keys().any(|k| is_billing(k)) {
		return Check::skip(
			CHECK_NAME,
			"no billing tags in canopy",
			"canopy tags carry no billing.* entries to reconcile against IMDS",
		);
	}

	let imds_tags = match fetch_imds_tags().await {
		Some(tags) => tags,
		None => {
			return Check::skip(
				CHECK_NAME,
				"no IMDS tags",
				"instance metadata tags unavailable (not on EC2, or IMDS tags disabled)",
			);
		}
	};

	let recon = reconcile(&canopy_tags, &imds_tags);
	recon.into_check()
}

fn is_billing(key: &str) -> bool {
	key.starts_with(BILLING_PREFIX)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Reconciliation {
	/// Billing keys present in canopy but absent from IMDS.
	missing: Vec<String>,
	/// Billing keys present in both, but with differing values.
	mismatched: Vec<Mismatch>,
}

#[derive(Debug, PartialEq, Eq)]
struct Mismatch {
	key: String,
	canopy: String,
	imds: String,
}

/// Compare the `billing.*` subset of `canopy` against `imds`.
///
/// Canopy is the source of truth for which billing tags should exist; IMDS is
/// where they must be reflected. Only keys present in canopy are considered —
/// billing-looking IMDS tags with no canopy counterpart are ignored, since
/// canopy governs the expected set.
fn reconcile(canopy: &BTreeMap<String, String>, imds: &BTreeMap<String, String>) -> Reconciliation {
	let mut recon = Reconciliation::default();
	for (key, canopy_value) in canopy.iter().filter(|(k, _)| is_billing(k)) {
		match imds.get(key) {
			None => recon.missing.push(key.clone()),
			Some(imds_value) if imds_value != canopy_value => recon.mismatched.push(Mismatch {
				key: key.clone(),
				canopy: canopy_value.clone(),
				imds: imds_value.clone(),
			}),
			Some(_) => {}
		}
	}
	recon
}

impl Reconciliation {
	fn into_check(self) -> Check {
		let missing_detail = Value::Array(
			self.missing
				.iter()
				.cloned()
				.map(Value::String)
				.collect::<Vec<_>>(),
		);
		let mismatched_detail = Value::Array(
			self.mismatched
				.iter()
				.map(|m| {
					serde_json::json!({
						"key": m.key,
						"canopy": m.canopy,
						"imds": m.imds,
					})
				})
				.collect::<Vec<_>>(),
		);

		let check = if !self.mismatched.is_empty() {
			let mut reasons: Vec<String> = self
				.mismatched
				.iter()
				.map(|m| format!("{} (canopy={}, imds={})", m.key, m.canopy, m.imds))
				.collect();
			if !self.missing.is_empty() {
				reasons.push(format!("missing from IMDS: {}", self.missing.join(", ")));
			}
			Check::fail(
				CHECK_NAME,
				format!("{} billing tag(s) mismatched", self.mismatched.len()),
				format!(
					"IMDS billing tags disagree with canopy: {}",
					reasons.join("; ")
				),
			)
		} else if !self.missing.is_empty() {
			Check::warning(
				CHECK_NAME,
				format!("{} billing tag(s) missing from IMDS", self.missing.len()),
				format!(
					"canopy billing tags not present on the instance: {}",
					self.missing.join(", ")
				),
			)
		} else {
			Check::pass(CHECK_NAME, "IMDS billing tags match canopy")
		};

		check
			.with_detail("missing", missing_detail)
			.with_detail("mismatched", mismatched_detail)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
		pairs
			.iter()
			.map(|(k, v)| (k.to_string(), v.to_string()))
			.collect()
	}

	#[test]
	fn all_present_and_matching_is_clean() {
		let canopy = map(&[("billing.customer", "acme"), ("role", "central")]);
		let imds = map(&[("billing.customer", "acme"), ("Name", "host-1")]);
		let recon = reconcile(&canopy, &imds);
		assert_eq!(recon, Reconciliation::default());
	}

	#[test]
	fn missing_billing_tag_is_reported() {
		let canopy = map(&[("billing.customer", "acme"), ("billing.project", "p1")]);
		let imds = map(&[("billing.customer", "acme")]);
		let recon = reconcile(&canopy, &imds);
		assert_eq!(recon.missing, vec!["billing.project".to_string()]);
		assert!(recon.mismatched.is_empty());
	}

	#[test]
	fn mismatched_billing_tag_is_reported() {
		let canopy = map(&[("billing.customer", "acme")]);
		let imds = map(&[("billing.customer", "globex")]);
		let recon = reconcile(&canopy, &imds);
		assert!(recon.missing.is_empty());
		assert_eq!(
			recon.mismatched,
			vec![Mismatch {
				key: "billing.customer".to_string(),
				canopy: "acme".to_string(),
				imds: "globex".to_string(),
			}]
		);
	}

	#[test]
	fn non_billing_tags_are_ignored() {
		let canopy = map(&[("role", "central"), ("fleet", "prod")]);
		let imds = map(&[("role", "facility")]);
		let recon = reconcile(&canopy, &imds);
		assert_eq!(recon, Reconciliation::default());
	}

	#[test]
	fn extra_imds_billing_tags_are_ignored() {
		let canopy = map(&[("billing.customer", "acme")]);
		let imds = map(&[("billing.customer", "acme"), ("billing.extra", "x")]);
		let recon = reconcile(&canopy, &imds);
		assert_eq!(recon, Reconciliation::default());
	}

	#[test]
	fn mismatch_takes_precedence_over_missing_as_failure() {
		let canopy = map(&[("billing.customer", "acme"), ("billing.project", "p1")]);
		let imds = map(&[("billing.customer", "globex")]);
		let recon = reconcile(&canopy, &imds);
		let check = recon.into_check();
		assert!(matches!(
			check.status,
			crate::doctor::check::CheckStatus::Fail(_)
		));
	}

	#[test]
	fn missing_only_is_a_warning() {
		let canopy = map(&[("billing.project", "p1")]);
		let imds = map(&[("billing.customer", "acme")]);
		let recon = reconcile(&canopy, &imds);
		let check = recon.into_check();
		assert!(matches!(
			check.status,
			crate::doctor::check::CheckStatus::Warning(_)
		));
	}

	#[test]
	fn clean_is_a_pass() {
		let canopy = map(&[("billing.customer", "acme")]);
		let imds = map(&[("billing.customer", "acme")]);
		let check = reconcile(&canopy, &imds).into_check();
		assert!(matches!(
			check.status,
			crate::doctor::check::CheckStatus::Pass
		));
	}
}
