//! Shared primitives for the `tamanu` lifecycle subcommands (`start`,
//! `stop`, `restart`, `status`).
//!
//! Discovery, matching, and supervisor (systemd/pm2) dispatch all live
//! here so the four subcommand entry points stay thin.

use miette::{Result, bail};

use super::services::Expectation;

/// Filter an expectation set by zero or more substring patterns.
///
/// - Empty `names`: returns every expectation unchanged.
/// - Otherwise: an expectation matches if **any** name in `names` is a
///   substring of the expectation's name.
///
/// Returns an error if any name in `names` matched zero expectations
/// (typo safety in multi-name invocations).
pub fn match_names<'a>(
	expectations: &'a [Expectation],
	names: &[&str],
) -> Result<Vec<&'a Expectation>> {
	if names.is_empty() {
		return Ok(expectations.iter().collect());
	}

	let unmatched: Vec<&str> = names
		.iter()
		.copied()
		.filter(|name| !expectations.iter().any(|e| e.name.contains(name)))
		.collect();
	if !unmatched.is_empty() {
		let available: Vec<&str> = expectations.iter().map(|e| e.name).collect();
		bail!(
			"no service matches: {}; available names are: {}",
			unmatched.join(", "),
			available.join(", "),
		);
	}

	let matched: Vec<&Expectation> = expectations
		.iter()
		.filter(|e| names.iter().any(|name| e.name.contains(name)))
		.collect();
	Ok(matched)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::actions::tamanu::services::{Criticality, ExpectedState, Instances};

	fn exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Up,
			criticality: Criticality::Background,
		}
	}

	#[test]
	fn empty_names_returns_everything() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks"), exp("tamanu-sync")];
		let m = match_names(&es, &[]).unwrap();
		assert_eq!(m.len(), 3);
	}

	#[test]
	fn single_name_substring_matches() {
		let es = [exp("tamanu-central-api"), exp("tamanu-central-tasks")];
		let m = match_names(&es, &["api"]).unwrap();
		assert_eq!(m.len(), 1);
		assert_eq!(m[0].name, "tamanu-central-api");
	}

	#[test]
	fn multi_name_union() {
		let es = [
			exp("tamanu-central-api"),
			exp("tamanu-central-tasks"),
			exp("tamanu-central-fhir-resolve"),
		];
		let m = match_names(&es, &["api", "fhir"]).unwrap();
		assert_eq!(m.len(), 2);
		assert_eq!(
			m.iter().map(|e| e.name).collect::<Vec<_>>(),
			vec!["tamanu-central-api", "tamanu-central-fhir-resolve"],
		);
	}

	#[test]
	fn zero_match_name_bails() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks")];
		let err = match_names(&es, &["nope"]).unwrap_err();
		let msg = format!("{err}");
		assert!(msg.contains("nope"), "error should name the bad pattern: {msg}");
		assert!(msg.contains("tamanu-api"), "error should list available: {msg}");
	}

	#[test]
	fn mixed_match_and_no_match_still_bails() {
		// One typo in a multi-name invocation should bail rather than silently
		// drop the bad pattern and process the rest.
		let es = [exp("tamanu-api"), exp("tamanu-tasks")];
		let err = match_names(&es, &["api", "nope"]).unwrap_err();
		let msg = format!("{err}");
		assert!(msg.contains("nope"), "error should name the bad pattern: {msg}");
	}

	#[test]
	fn name_substring_can_match_multiple() {
		let es = [
			exp("tamanu-central-fhir-resolve"),
			exp("tamanu-central-fhir-refresh"),
			exp("tamanu-api"),
		];
		let m = match_names(&es, &["fhir"]).unwrap();
		assert_eq!(m.len(), 2);
	}
}
