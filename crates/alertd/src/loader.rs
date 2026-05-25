use std::{collections::HashMap, path::Path};

use miette::Result;
use tracing::{debug, error, warn};
use walkdir::WalkDir;

use crate::{
	LogError,
	alert::{AlertDefinition, server_kind_matches},
	canopy::{DEFAULT_CANOPY_URL, Severity},
	glob_resolver::ResolvedPaths,
	targets::{AlertTargets, CanopyConfig, ExternalTarget, TargetCanopy, TargetConnection},
};

pub struct LoadedAlerts {
	pub alerts: Vec<(AlertDefinition, Vec<crate::targets::ResolvedTarget>)>,
	pub external_targets: HashMap<String, Vec<ExternalTarget>>,
	pub definition_errors: Vec<DefinitionError>,
}

#[derive(Debug, Clone)]
pub struct DefinitionError {
	pub file: std::path::PathBuf,
	pub error: String,
}

pub fn load_alerts_from_paths(
	resolved: &ResolvedPaths,
	canopy_available: bool,
	server_kind: Option<&str>,
) -> Result<LoadedAlerts> {
	let mut alerts = Vec::<AlertDefinition>::new();
	let mut external_targets = HashMap::new();
	let mut definition_errors = Vec::new();

	// Load external targets from files
	for external_targets_path in &resolved.files {
		if let Some(name) = external_targets_path.file_name()
			&& (name.eq_ignore_ascii_case("_targets.yml")
				|| name.eq_ignore_ascii_case("_targets.yaml"))
			&& let Some(AlertTargets { targets }) = std::fs::read_to_string(external_targets_path)
				.ok()
				.and_then(|content| {
					debug!(path=?external_targets_path, "parsing external targets");
					serde_yaml::from_str::<AlertTargets>(&content)
						.map_err(
							|err| warn!(path=?external_targets_path, "_targets.yml has errors! {err}"),
						)
						.ok()
				}) {
			debug!(path=?external_targets_path, count=targets.len(), "loaded external targets from file");
			for target in targets {
				debug!(id=%target.id, path=?external_targets_path, "adding external target");
				external_targets
					.entry(target.id.clone())
					.or_insert(Vec::new())
					.push(target);
			}
		}
	}

	// Load external targets from directories
	for dir in &resolved.dirs {
		for external_targets_path in [dir.join("_targets.yml"), dir.join("_targets.yaml")] {
			if let Some(AlertTargets { targets }) = std::fs::read_to_string(&external_targets_path)
				.ok()
				.and_then(|content| {
					debug!(path=?external_targets_path, "parsing external targets");
					serde_yaml::from_str::<AlertTargets>(&content)
						.map_err(
							|err| warn!(path=?external_targets_path, "_targets.yml has errors! {err}"),
						)
						.ok()
				}) {
				debug!(path=?external_targets_path, count=targets.len(), "loaded external targets from directory");
				for target in targets {
					debug!(id=%target.id, path=?external_targets_path, "adding external target");
					external_targets
						.entry(target.id.clone())
						.or_insert(Vec::new())
						.push(target);
				}
			}
		}
	}

	// Load alerts from directories (recursively)
	for dir in &resolved.dirs {
		for entry in WalkDir::new(dir)
			.into_iter()
			.filter_map(|e| e.ok())
			.filter(|e| e.file_type().is_file())
		{
			match load_alert_from_file(entry.path()) {
				LoadAlertResult::Success(alert) => {
					push_if_targeted(&mut alerts, alert, server_kind)
				}
				LoadAlertResult::Error(err) => definition_errors.push(err),
				LoadAlertResult::Disabled | LoadAlertResult::Skip => {}
			}
		}
	}

	// Load alerts from individual files
	for file in &resolved.files {
		match load_alert_from_file(file) {
			LoadAlertResult::Success(alert) => push_if_targeted(&mut alerts, alert, server_kind),
			LoadAlertResult::Error(err) => definition_errors.push(err),
			LoadAlertResult::Disabled | LoadAlertResult::Skip => {}
		}
	}

	if !external_targets.is_empty() {
		debug!(
			count=%external_targets.len(),
			ids=?external_targets.keys().collect::<Vec<_>>(),
			"found external targets"
		);
	} else {
		warn!("no external targets found");
	}

	// If no `default` target was explicitly configured and canopy auth is
	// available, register a synthesised canopy target under "default" so
	// alerts that reference `id: default` (and the event-manager fallback)
	// route to canopy automatically.
	if canopy_available && !external_targets.contains_key("default") {
		debug!("no 'default' target configured, synthesising canopy default");
		external_targets.insert(
			"default".to_string(),
			vec![ExternalTarget {
				id: "default".to_string(),
				conn: TargetConnection::Canopy(TargetCanopy {
					canopy: CanopyConfig {
						url: DEFAULT_CANOPY_URL
							.parse()
							.expect("default canopy URL is valid"),
						source: "bestool-alertd".to_string(),
						severity: Some(Severity::Error),
					},
				}),
			}],
		);
	}

	let alerts_with_targets: Vec<_> = alerts
		.into_iter()
		.filter_map(|alert| {
			let file = alert.file.clone();
			let send_target_ids: Vec<_> = alert.send.iter().map(|t| t.id()).collect();
			debug!(
				file=?file,
				send_targets=?send_target_ids,
				available_targets=?external_targets.keys().collect::<Vec<_>>(),
				"normalising alert"
			);
			match alert.normalise(&external_targets) {
				Ok(normalized) => Some(normalized),
				Err(err) => {
					error!(file=?file, "failed to normalise alert: {}", LogError(&err));
					definition_errors.push(DefinitionError {
						file: file.clone(),
						error: format!("{:#}", err),
					});
					None
				}
			}
		})
		.collect();

	debug!(count=%alerts_with_targets.len(), "found some alerts");

	if !definition_errors.is_empty() {
		warn!(count=%definition_errors.len(), "found alert definition errors");
	}

	Ok(LoadedAlerts {
		alerts: alerts_with_targets,
		external_targets,
		definition_errors,
	})
}

/// Push an alert onto the accumulator iff its `server-kind:` matches the
/// daemon's configured server kind. Logs the drop at debug so an operator
/// wondering where a facility-only alert went can spot it in trace output.
fn push_if_targeted(
	alerts: &mut Vec<AlertDefinition>,
	alert: AlertDefinition,
	server_kind: Option<&str>,
) {
	if server_kind_matches(alert.server_kind.as_deref(), server_kind) {
		alerts.push(alert);
	} else {
		debug!(
			file = %alert.file.display(),
			alert_kind = ?alert.server_kind,
			daemon_kind = ?server_kind,
			"skipping alert: server-kind does not match daemon"
		);
	}
}

enum LoadAlertResult {
	Success(AlertDefinition),
	Disabled,
	Skip,
	Error(DefinitionError),
}

fn load_alert_from_file(file: &Path) -> LoadAlertResult {
	if !file.extension().is_some_and(|e| e == "yaml" || e == "yml") {
		return LoadAlertResult::Skip;
	}

	if file.file_stem().is_some_and(|n| n == "_targets") {
		return LoadAlertResult::Skip;
	}

	debug!(?file, "parsing YAML file");
	let content = match std::fs::read_to_string(file) {
		Ok(content) => content,
		Err(err) => {
			error!(?file, "failed to read file: {err}");
			return LoadAlertResult::Error(DefinitionError {
				file: file.to_path_buf(),
				error: format!("Failed to read file: {}", err),
			});
		}
	};

	let mut alert: AlertDefinition = match serde_yaml::from_str(&content) {
		Ok(alert) => alert,
		Err(err) => {
			error!(?file, "failed to parse YAML: {err}");
			return LoadAlertResult::Error(DefinitionError {
				file: file.to_path_buf(),
				error: format!("Failed to parse YAML: {}", err),
			});
		}
	};

	alert.file = file.to_path_buf();
	debug!(?alert, "parsed alert file");

	if alert.enabled {
		LoadAlertResult::Success(alert)
	} else {
		LoadAlertResult::Disabled
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;

	fn empty_resolved(dir: &Path) -> ResolvedPaths {
		ResolvedPaths {
			dirs: vec![dir.to_path_buf()],
			files: vec![],
		}
	}

	#[test]
	fn canopy_default_injected_when_no_targets_and_canopy_available() {
		let tmp = TempDir::new().unwrap();
		let resolved = empty_resolved(tmp.path());

		let loaded = load_alerts_from_paths(&resolved, true, None).unwrap();
		assert!(loaded.external_targets.contains_key("default"));
		let default = &loaded.external_targets["default"][0];
		assert_eq!(default.id, "default");
		assert!(matches!(default.conn, TargetConnection::Canopy(_)));
	}

	#[test]
	fn no_canopy_default_when_canopy_unavailable() {
		let tmp = TempDir::new().unwrap();
		let resolved = empty_resolved(tmp.path());

		let loaded = load_alerts_from_paths(&resolved, false, None).unwrap();
		assert!(loaded.external_targets.is_empty());
	}

	#[test]
	fn explicit_default_takes_precedence_over_canopy_synth() {
		let tmp = TempDir::new().unwrap();
		std::fs::write(
			tmp.path().join("_targets.yml"),
			r#"
targets:
  - id: default
    addresses: [team@example.com]
"#,
		)
		.unwrap();
		let resolved = empty_resolved(tmp.path());

		let loaded = load_alerts_from_paths(&resolved, true, None).unwrap();
		let default = &loaded.external_targets["default"][0];
		// User's explicit email default wins; no canopy injection.
		assert!(matches!(default.conn, TargetConnection::Email(_)));
		assert_eq!(loaded.external_targets["default"].len(), 1);
	}

	#[test]
	fn alert_referencing_default_resolves_to_synth_canopy() {
		let tmp = TempDir::new().unwrap();
		std::fs::write(
			tmp.path().join("disk.yml"),
			r#"
sql: "SELECT 1"
send:
  - id: default
    subject: "Test"
    template: "Body"
"#,
		)
		.unwrap();
		let resolved = empty_resolved(tmp.path());

		let loaded = load_alerts_from_paths(&resolved, true, None).unwrap();
		assert_eq!(loaded.alerts.len(), 1);
		let (_, resolved_targets) = &loaded.alerts[0];
		assert_eq!(resolved_targets.len(), 1);
		assert!(matches!(
			resolved_targets[0].conn,
			TargetConnection::Canopy(_)
		));
	}

	fn write_alert(dir: &Path, name: &str, server_kind: Option<&str>) {
		let server_kind_line = server_kind
			.map(|t| format!("server-kind: {t}\n"))
			.unwrap_or_default();
		std::fs::write(
			dir.join(name),
			format!(
				"sql: \"SELECT 1\"\n\
				send:\n  - id: default\n    subject: \"x\"\n    template: \"y\"\n\
				{server_kind_line}"
			),
		)
		.unwrap();
	}

	#[test]
	fn target_filter_keeps_matching_alerts_only() {
		let tmp = TempDir::new().unwrap();
		write_alert(tmp.path(), "central-only.yml", Some("central"));
		write_alert(tmp.path(), "facility-only.yml", Some("facility"));
		write_alert(tmp.path(), "kiosk-only.yml", Some("kiosk"));
		write_alert(tmp.path(), "no-target.yml", None);
		let resolved = empty_resolved(tmp.path());

		let loaded = load_alerts_from_paths(&resolved, true, Some("central")).unwrap();
		let kept: Vec<String> = loaded
			.alerts
			.iter()
			.map(|(a, _)| a.file.file_name().unwrap().to_string_lossy().into_owned())
			.collect();
		assert!(kept.contains(&"central-only.yml".into()));
		assert!(kept.contains(&"no-target.yml".into()));
		assert!(!kept.contains(&"facility-only.yml".into()));
		assert!(
			!kept.contains(&"kiosk-only.yml".into()),
			"alertd matches the daemon's server_kind by string equality; unrelated kinds are dropped"
		);
	}

	#[test]
	fn target_filter_absent_kind_admits_everything() {
		// alertd running with no `server_kind` configured (e.g. outside a
		// Tamanu install) shouldn't silently swallow targeted alerts.
		let tmp = TempDir::new().unwrap();
		write_alert(tmp.path(), "central-only.yml", Some("central"));
		write_alert(tmp.path(), "facility-only.yml", Some("facility"));
		let resolved = empty_resolved(tmp.path());

		let loaded = load_alerts_from_paths(&resolved, true, None).unwrap();
		assert_eq!(loaded.alerts.len(), 2);
	}
}
