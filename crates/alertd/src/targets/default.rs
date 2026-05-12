use std::collections::HashMap;

use tracing::debug;

use crate::{
	ExternalTarget,
	canopy::{DEFAULT_CANOPY_URL, Severity},
	targets::{TargetCanopy, TargetConnection, canopy::CanopyConfig},
};

/// Pick the fallback target for system-level (synthetic) events.
///
/// If `_targets.yml` defines targets, picks among them using the usual rules
/// (single → use it, named `default` → use it, else first alphabetical).
///
/// If `_targets.yml` is missing or empty AND canopy auth is available, returns
/// a synthesised canopy target so internal events (database-down, alert load
/// failures, etc.) still go somewhere visible.
pub fn determine_default_target(
	external_targets: &HashMap<String, Vec<ExternalTarget>>,
	canopy_available: bool,
) -> Option<ExternalTarget> {
	if external_targets.is_empty() {
		if canopy_available {
			debug!("no _targets configured; using synthesised canopy default");
			return Some(synthesise_canopy_default());
		}
		return None;
	}

	// If there's only one target, use it
	if external_targets.len() == 1 {
		let (id, targets) = external_targets.iter().next().unwrap();
		if let Some(t) = targets.first() {
			debug!(id, "using only available target as default");
			return Some(t.clone());
		}
	}

	// If there's a target named "default", use that
	if let Some(targets) = external_targets.get("default")
		&& let Some(t) = targets.first()
	{
		debug!("using 'default' target");
		return Some(t.clone());
	}

	// Otherwise, use the first target alphabetically
	let mut sorted_ids: Vec<_> = external_targets.keys().collect();
	sorted_ids.sort();
	if let Some(id) = sorted_ids.first()
		&& let Some(targets) = external_targets.get(*id)
		&& let Some(t) = targets.first()
	{
		debug!(id, "using first alphabetical target as default");
		return Some(t.clone());
	}

	None
}

fn synthesise_canopy_default() -> ExternalTarget {
	ExternalTarget {
		id: "_canopy_default".to_string(),
		conn: TargetConnection::Canopy(TargetCanopy {
			canopy: CanopyConfig {
				url: DEFAULT_CANOPY_URL
					.parse()
					.expect("default canopy URL is valid"),
				source: "bestool-alertd".to_string(),
				severity: Some(Severity::Error),
			},
		}),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::targets::{TargetConnection, TargetEmail};

	#[test]
	fn test_determine_default_target_single() {
		let mut targets = HashMap::new();
		targets.insert(
			"only-target".to_string(),
			vec![ExternalTarget {
				id: "only-target".to_string(),
				conn: TargetConnection::Email(TargetEmail {
					addresses: vec!["test@example.com".to_string()],
				}),
			}],
		);

		let default = determine_default_target(&targets, false);
		assert!(default.is_some());
	}

	#[test]
	fn test_determine_default_target_named_default() {
		let mut targets = HashMap::new();
		targets.insert(
			"default".to_string(),
			vec![ExternalTarget {
				id: "default".to_string(),
				conn: TargetConnection::Email(TargetEmail {
					addresses: vec!["default@example.com".to_string()],
				}),
			}],
		);
		targets.insert(
			"other".to_string(),
			vec![ExternalTarget {
				id: "other".to_string(),
				conn: TargetConnection::Email(TargetEmail {
					addresses: vec!["other@example.com".to_string()],
				}),
			}],
		);

		let default = determine_default_target(&targets, false);
		assert!(default.is_some());
		let default = default.unwrap();
		match &default.conn {
			TargetConnection::Email(email) => assert_eq!(email.addresses[0], "default@example.com"),
			_ => panic!("expected email target"),
		}
	}

	#[test]
	fn test_determine_default_target_alphabetical() {
		let mut targets = HashMap::new();
		targets.insert(
			"zebra".to_string(),
			vec![ExternalTarget {
				id: "zebra".to_string(),
				conn: TargetConnection::Email(TargetEmail {
					addresses: vec!["zebra@example.com".to_string()],
				}),
			}],
		);
		targets.insert(
			"alpha".to_string(),
			vec![ExternalTarget {
				id: "alpha".to_string(),
				conn: TargetConnection::Email(TargetEmail {
					addresses: vec!["alpha@example.com".to_string()],
				}),
			}],
		);

		let default = determine_default_target(&targets, false);
		assert!(default.is_some());
		let default = default.unwrap();
		match &default.conn {
			TargetConnection::Email(email) => assert_eq!(email.addresses[0], "alpha@example.com"),
			_ => panic!("expected email target"),
		}
	}

	#[test]
	fn test_synthesised_canopy_default_when_no_targets() {
		let targets = HashMap::new();
		let default = determine_default_target(&targets, true);
		assert!(default.is_some());
		let default = default.unwrap();
		assert_eq!(default.id, "_canopy_default");
		assert!(matches!(default.conn, TargetConnection::Canopy(_)));
	}

	#[test]
	fn test_no_default_when_no_targets_and_no_canopy() {
		let targets = HashMap::new();
		let default = determine_default_target(&targets, false);
		assert!(default.is_none());
	}

	#[test]
	fn test_explicit_targets_take_precedence_over_canopy_default() {
		let mut targets = HashMap::new();
		targets.insert(
			"team".to_string(),
			vec![ExternalTarget {
				id: "team".to_string(),
				conn: TargetConnection::Email(TargetEmail {
					addresses: vec!["team@example.com".to_string()],
				}),
			}],
		);

		let default = determine_default_target(&targets, true);
		let default = default.unwrap();
		assert!(matches!(default.conn, TargetConnection::Email(_)));
	}
}
