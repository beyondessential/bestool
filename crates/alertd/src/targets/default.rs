use std::collections::HashMap;

use tracing::debug;

use crate::{ExternalTarget, targets::TargetConnection};

/// Pick the fallback target for system-level (synthetic) events.
///
/// Canopy targets are skipped: synthetic events need a rendered subject/body
/// and don't map cleanly to canopy's per-source dedup model, so we prefer to
/// surface them via email/slack and let canopy continue to receive only
/// alert-triggered events.
pub fn determine_default_target(
	external_targets: &HashMap<String, Vec<ExternalTarget>>,
) -> Option<&ExternalTarget> {
	fn first_non_canopy(targets: &[ExternalTarget]) -> Option<&ExternalTarget> {
		targets
			.iter()
			.find(|t| !matches!(t.conn, TargetConnection::Canopy(_)))
	}

	if external_targets.is_empty() {
		return None;
	}

	// If there's only one usable target, use it
	if external_targets.len() == 1 {
		let (id, targets) = external_targets.iter().next().unwrap();
		if let Some(t) = first_non_canopy(targets) {
			debug!(id, "using only available target as default");
			return Some(t);
		}
	}

	// If there's a target named "default", use that
	if let Some(targets) = external_targets.get("default")
		&& let Some(t) = first_non_canopy(targets)
	{
		debug!("using 'default' target");
		return Some(t);
	}

	// Otherwise, use the first target alphabetically that isn't canopy
	let mut sorted_ids: Vec<_> = external_targets.keys().collect();
	sorted_ids.sort();
	for id in sorted_ids {
		if let Some(targets) = external_targets.get(id)
			&& let Some(t) = first_non_canopy(targets)
		{
			debug!(id, "using first alphabetical non-canopy target as default");
			return Some(t);
		}
	}

	None
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

		let default = determine_default_target(&targets);
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

		let default = determine_default_target(&targets);
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

		let default = determine_default_target(&targets);
		assert!(default.is_some());
		let default = default.unwrap();
		match &default.conn {
			TargetConnection::Email(email) => assert_eq!(email.addresses[0], "alpha@example.com"),
			_ => panic!("expected email target"),
		}
	}
}
