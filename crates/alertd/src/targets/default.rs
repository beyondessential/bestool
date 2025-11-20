use std::collections::HashMap;

use tracing::debug;

use crate::ExternalTarget;

pub fn determine_default_target(
	external_targets: &HashMap<String, Vec<ExternalTarget>>,
) -> Option<&ExternalTarget> {
	if external_targets.is_empty() {
		return None;
	}

	// If there's only one target, use it
	if external_targets.len() == 1 {
		let (id, targets) = external_targets.iter().next().unwrap();
		debug!(id, "using only available target as default");
		return targets.first();
	}

	// If there's a target named "default", use that
	if let Some(targets) = external_targets.get("default") {
		debug!("using 'default' target");
		return targets.first();
	}

	// Otherwise, use the first target alphabetically
	let mut sorted_ids: Vec<_> = external_targets.keys().collect();
	sorted_ids.sort();
	if let Some(id) = sorted_ids.first() {
		debug!(id, "using first alphabetical target as default");
		if let Some(targets) = external_targets.get(*id) {
			return targets.first();
		}
	}

	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::targets::TargetEmail;

	#[test]
	fn test_determine_default_target_single() {
		let mut targets = HashMap::new();
		targets.insert(
			"only-target".to_string(),
			vec![ExternalTarget {
				id: "only-target".to_string(),
				conn: TargetEmail {
					addresses: vec!["test@example.com".to_string()],
				},
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
				conn: TargetEmail {
					addresses: vec!["default@example.com".to_string()],
				},
			}],
		);
		targets.insert(
			"other".to_string(),
			vec![ExternalTarget {
				id: "other".to_string(),
				conn: TargetEmail {
					addresses: vec!["other@example.com".to_string()],
				},
			}],
		);

		let default = determine_default_target(&targets);
		assert!(default.is_some());
		let default = default.unwrap();
		assert_eq!(default.conn.addresses[0], "default@example.com");
	}

	#[test]
	fn test_determine_default_target_alphabetical() {
		let mut targets = HashMap::new();
		targets.insert(
			"zebra".to_string(),
			vec![ExternalTarget {
				id: "zebra".to_string(),
				conn: TargetEmail {
					addresses: vec!["zebra@example.com".to_string()],
				},
			}],
		);
		targets.insert(
			"alpha".to_string(),
			vec![ExternalTarget {
				id: "alpha".to_string(),
				conn: TargetEmail {
					addresses: vec!["alpha@example.com".to_string()],
				},
			}],
		);

		let default = determine_default_target(&targets);
		assert!(default.is_some());
		let default = default.unwrap();
		assert_eq!(default.conn.addresses[0], "alpha@example.com");
	}
}
